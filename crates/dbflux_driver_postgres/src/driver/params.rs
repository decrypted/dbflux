//! Driver parameters of a PostgreSQL connection. They are appended to the
//! libpq-style connection string (keyword or URI form) after everything the
//! form and DBFlux put there, and `postgres::Config` applies options in order,
//! so an explicit parameter wins over a URI query option and over the client
//! identity DBFlux sends as `application_name`.

use dbflux_core::{
    ConnectionProfile, DbError, DriverParam, DriverParamKey, DriverParamsSpec, driver_params_map,
    refuse_form_owned_params,
};

use super::escape_keyword_value;

pub(crate) static POSTGRES_DRIVER_PARAMS: DriverParamsSpec = DriverParamsSpec {
    keys: &[
        DriverParamKey {
            key: "application_name",
            example: "reporting",
        },
        DriverParamKey {
            key: "options",
            example: "-c statement_timeout=5000",
        },
        DriverParamKey {
            key: "connect_timeout",
            example: "10",
        },
        DriverParamKey {
            key: "tcp_user_timeout",
            example: "30",
        },
        DriverParamKey {
            key: "keepalives",
            example: "1",
        },
        DriverParamKey {
            key: "keepalives_idle",
            example: "60",
        },
        DriverParamKey {
            key: "keepalives_interval",
            example: "10",
        },
        DriverParamKey {
            key: "keepalives_retries",
            example: "3",
        },
        DriverParamKey {
            key: "target_session_attrs",
            example: "any | read-write | read-only",
        },
        DriverParamKey {
            key: "channel_binding",
            example: "disable | prefer | require",
        },
    ],
    compression: None,
};

/// Keys the connection form owns: address, credentials, database and every
/// TLS setting (the Transport section sets those).
fn is_form_owned(key: &str) -> bool {
    matches!(
        key,
        "host" | "hostaddr" | "port" | "user" | "password" | "dbname" | "database"
    ) || key.starts_with("ssl")
}

/// Validated driver parameters of one PostgreSQL connection, sorted by key so
/// the generated connection string is stable.
#[derive(Debug, Clone, Default)]
pub(crate) struct PgDriverParams {
    pairs: Vec<(String, String)>,
}

impl PgDriverParams {
    pub(crate) fn from_profile(profile: &ConnectionProfile) -> Result<Self, DbError> {
        Self::resolve(&profile.driver_params)
    }

    pub(crate) fn resolve(params: &[DriverParam]) -> Result<Self, DbError> {
        let map = driver_params_map(params);
        refuse_form_owned_params("PostgreSQL", &map, is_form_owned)?;

        let mut pairs: Vec<(String, String)> = map.into_iter().collect();
        pairs.sort();

        for (key, value) in &pairs {
            if !POSTGRES_DRIVER_PARAMS
                .keys
                .iter()
                .any(|known| known.key == key)
            {
                return Err(DbError::InvalidProfile(format!(
                    "Unknown PostgreSQL driver parameter `{key}`. Supported keys: {}",
                    supported_keys()
                )));
            }

            // The client's own parser decides whether the value is valid.
            format!("{key}={}", escape_keyword_value(value))
                .parse::<postgres::Config>()
                .map_err(|_error| {
                    DbError::InvalidProfile(format!(
                        "Invalid value `{value}` for PostgreSQL driver parameter `{key}`"
                    ))
                })?;
        }

        Ok(Self { pairs })
    }

    /// Appends the parameters to a keyword/value connection string.
    pub(crate) fn append_to_keyword(&self, conn_string: &str) -> String {
        let mut result = conn_string.to_string();
        for (key, value) in &self.pairs {
            result.push(' ');
            result.push_str(key);
            result.push('=');
            result.push_str(&escape_keyword_value(value));
        }
        result
    }

    /// Appends the parameters as query options of a connection URI.
    pub(crate) fn append_to_uri(&self, uri: &str) -> String {
        let mut result = uri.to_string();
        for (key, value) in &self.pairs {
            result.push(if result.contains('?') { '&' } else { '?' });
            result.push_str(key);
            result.push('=');
            result.push_str(&urlencoding::encode(value));
        }
        result
    }
}

fn supported_keys() -> String {
    POSTGRES_DRIVER_PARAMS
        .keys
        .iter()
        .map(|key| key.key)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;
