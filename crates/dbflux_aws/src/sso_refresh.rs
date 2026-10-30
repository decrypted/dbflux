//! Silent renewal of IAM Identity Center access tokens cached by `aws sso login`.
//!
//! An `sso-session` login caches an access token that lives one hour next to a
//! refresh token and an OIDC client registration that live much longer. The
//! AWS CLI and SDKs renew the access token from those without a browser, so
//! DBFlux does the same before it asks for a login, and writes the result back
//! to the same cache file so the CLI and DBFlux keep sharing one session.

use std::io::Write;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};

use crate::auth::{parse_sso_expiry, run_on_dedicated_thread};

/// The inputs of an SSO-OIDC `CreateToken` call with the `refresh_token` grant.
pub(crate) struct SsoRefreshRequest {
    pub(crate) region: String,
    pub(crate) client_id: String,
    pub(crate) client_secret: SecretString,
    pub(crate) refresh_token: SecretString,
}

impl SsoRefreshRequest {
    /// Returns `None` when the cache entry cannot be refreshed: logins of
    /// legacy profiles (`sso_start_url` without `sso_session`) cache no refresh
    /// token, and the OIDC service rejects the grant once the client
    /// registration has expired.
    pub(crate) fn from_cache_entry(entry: &serde_json::Value, now: DateTime<Utc>) -> Option<Self> {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };

        let registration_expires_at = text("registrationExpiresAt").and_then(parse_sso_expiry)?;
        if registration_expires_at <= now {
            return None;
        }

        Some(Self {
            region: text("region")?.to_string(),
            client_id: text("clientId")?.to_string(),
            client_secret: SecretString::from(text("clientSecret")?.to_string()),
            refresh_token: SecretString::from(text("refreshToken")?.to_string()),
        })
    }
}

/// A new access token from `CreateToken`.
pub(crate) struct RefreshedSsoToken {
    pub(crate) access_token: SecretString,
    pub(crate) expires_in_secs: i64,
    /// Set when the service rotated the refresh token; the old one may no
    /// longer be accepted, so it must replace the cached one.
    pub(crate) refresh_token: Option<SecretString>,
}

/// Seam for the network call, so validation can be tested without AWS.
#[async_trait]
pub(crate) trait SsoTokenRefresher: Send + Sync {
    /// The error is a message for the log; it must never contain a token or
    /// the client secret.
    async fn refresh(&self, request: SsoRefreshRequest) -> Result<RefreshedSsoToken, String>;
}

/// Calls the SSO-OIDC service through the AWS SDK.
#[derive(Default)]
pub(crate) struct OidcSsoTokenRefresher {
    endpoint_url: Option<String>,
}

impl OidcSsoTokenRefresher {
    #[cfg(test)]
    pub(crate) fn with_endpoint_url(endpoint_url: impl Into<String>) -> Self {
        Self {
            endpoint_url: Some(endpoint_url.into()),
        }
    }
}

#[async_trait]
impl SsoTokenRefresher for OidcSsoTokenRefresher {
    async fn refresh(&self, request: SsoRefreshRequest) -> Result<RefreshedSsoToken, String> {
        let endpoint_url = self.endpoint_url.clone();

        // Validation runs on the GPUI executor, which has no Tokio reactor
        // for the SDK's HTTP client.
        run_on_dedicated_thread(move || refresh_sso_token_blocking(request, endpoint_url))
            .await
            .unwrap_or_else(|| Err("the SSO token refresh thread terminated".to_string()))
    }
}

fn refresh_sso_token_blocking(
    request: SsoRefreshRequest,
    endpoint_url: Option<String>,
) -> Result<RefreshedSsoToken, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|err| format!("failed to create a Tokio runtime: {}", err))?;

    runtime.block_on(async move {
        // Same configuration aws-config uses for its own refresh: the default
        // chain (proxy, endpoint and FIPS settings) with the cached SSO region.
        // CreateToken is unsigned, so no credentials are resolved.
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(request.region.clone()))
            .no_credentials();

        if let Some(url) = endpoint_url {
            loader = loader.endpoint_url(url);
        }

        let sdk_config = loader.load().await;

        let output = aws_sdk_ssooidc::Client::new(&sdk_config)
            .create_token()
            .grant_type("refresh_token")
            .client_id(request.client_id)
            .client_secret(request.client_secret.expose_secret())
            .refresh_token(request.refresh_token.expose_secret())
            .send()
            .await
            .map_err(|err| aws_sdk_ssooidc::error::DisplayErrorContext(&err).to_string())?;

        let access_token = output
            .access_token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| "CreateToken returned no access token".to_string())?;

        if output.expires_in <= 0 {
            return Err(format!(
                "CreateToken returned an invalid lifetime of {}s",
                output.expires_in
            ));
        }

        Ok(RefreshedSsoToken {
            access_token: SecretString::from(access_token),
            expires_in_secs: i64::from(output.expires_in),
            refresh_token: output
                .refresh_token
                .filter(|token| !token.is_empty())
                .map(SecretString::from),
        })
    })
}

/// Stores a refreshed token in the cache entry it was refreshed from.
///
/// Keys DBFlux does not change (client registration, region, start URL and
/// anything a newer CLI adds) are kept, so the CLI keeps reading the file.
pub(crate) fn write_refreshed_cache_entry(
    path: &Path,
    entry: &serde_json::Value,
    refreshed: &RefreshedSsoToken,
    expires_at: DateTime<Utc>,
) -> std::io::Result<()> {
    let mut updated = entry.clone();
    let object = updated.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the SSO cache entry is not a JSON object",
        )
    })?;

    object.insert(
        "accessToken".to_string(),
        serde_json::Value::String(refreshed.access_token.expose_secret().to_string()),
    );
    // The CLI writes and expects whole seconds in UTC with a `Z` suffix.
    object.insert(
        "expiresAt".to_string(),
        serde_json::Value::String(expires_at.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
    );
    if let Some(refresh_token) = &refreshed.refresh_token {
        object.insert(
            "refreshToken".to_string(),
            serde_json::Value::String(refresh_token.expose_secret().to_string()),
        );
    }

    let contents = serde_json::to_vec(&updated).map_err(std::io::Error::other)?;
    replace_file_atomically(path, &contents)
}

/// Replaces `path` through a temporary file in the same directory, so a
/// concurrent reader (the CLI, the SDK or another connect) never sees a
/// truncated cache file, and keeps the permissions of the file it replaces.
fn replace_file_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let invalid_path = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid SSO cache path '{}'", path.display()),
        )
    };
    let dir = path.parent().ok_or_else(invalid_path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid_path)?;

    let permissions = std::fs::metadata(path)?.permissions();

    // The `.tmp` extension keeps the file out of the `.json` cache scans.
    let temp_path = dir.join(format!(
        ".{}.{}.tmp",
        file_name,
        uuid::Uuid::new_v4().simple()
    ));

    let result = write_then_rename(&temp_path, path, contents, permissions);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn write_then_rename(
    temp_path: &Path,
    target: &Path,
    contents: &[u8],
    permissions: std::fs::Permissions,
) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);

    // The file holds tokens: it must not be readable by others even before
    // the original permissions are copied onto it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(temp_path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);

    std::fs::set_permissions(temp_path, permissions)?;
    std::fs::rename(temp_path, target)
}
