use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::OnceLock;

pub const APP_CONTROL_AUTH_TOKEN_ENV: &str = "DBFLUX_IPC_TOKEN";
pub const DRIVER_RPC_AUTH_TOKEN_ENV: &str = "DBFLUX_DRIVER_IPC_TOKEN";
pub const AUTH_PROVIDER_RPC_AUTH_TOKEN_ENV: &str = "DBFLUX_AUTH_PROVIDER_IPC_TOKEN";
pub const SECRET_PROVIDER_RPC_AUTH_TOKEN_ENV: &str = "DBFLUX_SECRET_PROVIDER_RPC_AUTH_TOKEN";

const AUTH_TOKEN_FILE: &str = "ipc_auth_token";

/// Process-global IPC auth token store.
///
/// Replaces delivery through the process environment: the token is set once by
/// `init_process_auth_tokens()` and read through `process_auth_token()`, so no
/// environment mutation is needed after threads exist.
static PROCESS_AUTH_TOKEN: OnceLock<String> = OnceLock::new();

pub fn init_process_auth_tokens() -> io::Result<String> {
    // Idempotent: the store decides the token, so a repeated call returns the
    // token already in force and rewrites the same bytes to the file instead of
    // diverging from a freshly generated one.
    let token = PROCESS_AUTH_TOKEN
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .clone();

    write_app_control_token(&token)?;
    Ok(token)
}

/// Returns the process-global IPC auth token set by `init_process_auth_tokens()`.
///
/// `None` when the store was never initialized in this process (e.g. the
/// standalone `dbflux mcp` server); readers then fall back to the
/// caller-supplied token environment variables.
pub fn process_auth_token() -> Option<&'static str> {
    PROCESS_AUTH_TOKEN.get().map(String::as_str)
}

pub fn read_app_control_token() -> io::Result<String> {
    let path = app_control_token_path()?;
    let token = fs::read_to_string(path)?;
    Ok(token.trim().to_string())
}

pub fn write_app_control_token(token: &str) -> io::Result<()> {
    let path = app_control_token_path()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);

    // Created owner-only, so the token is never readable under the umask mode
    // for the moment before the chmod below.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    options.open(&path)?.write_all(token.as_bytes())?;

    // The creation mode does not apply to a file that already exists.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&path, permissions)?;
    }

    Ok(())
}

/// Compares a presented IPC token with the expected one in constant time, so
/// response timing does not reveal how many leading bytes were right. An empty
/// expected token never matches: it means none was provisioned.
pub fn tokens_match(presented: Option<&str>, expected: &str) -> bool {
    use subtle::ConstantTimeEq;

    !expected.is_empty()
        && presented
            .is_some_and(|presented| bool::from(presented.as_bytes().ct_eq(expected.as_bytes())))
}

pub fn app_control_token_path() -> io::Result<PathBuf> {
    let dir = dbflux_storage::paths::data_dir().map_err(|e| io::Error::other(e.to_string()))?;
    Ok(dir.join(AUTH_TOKEN_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_match_only_the_exact_token() {
        let expected = "3f2b8c1e-0d4a-4e6f-9b7a-1c2d3e4f5a6b";

        assert!(tokens_match(Some(expected), expected));
        assert!(!tokens_match(None, expected));
        assert!(!tokens_match(Some(""), expected));
        // Same length, one byte off at each end.
        assert!(!tokens_match(
            Some("4f2b8c1e-0d4a-4e6f-9b7a-1c2d3e4f5a6b"),
            expected
        ));
        assert!(!tokens_match(
            Some("3f2b8c1e-0d4a-4e6f-9b7a-1c2d3e4f5a6c"),
            expected
        ));
        // A prefix or an extension must not pass, the classic zip-compare bug.
        assert!(!tokens_match(Some(&expected[..10]), expected));
        assert!(!tokens_match(Some(&format!("{expected}x")), expected));
        assert!(!tokens_match(Some(expected), ""));
        assert!(!tokens_match(Some(""), ""));
    }

    #[test]
    fn app_control_token_path_is_under_data_dir() {
        let token_path = app_control_token_path().expect("must resolve token path");
        let data_dir = dbflux_storage::paths::data_dir().expect("must resolve data dir");

        assert_eq!(
            token_path.parent().expect("token path must have a parent"),
            data_dir,
            "ipc_auth_token must be located under the data directory"
        );
        assert_eq!(
            token_path.file_name().and_then(|n| n.to_str()),
            Some("ipc_auth_token"),
            "token file must be named 'ipc_auth_token'"
        );
    }
}
