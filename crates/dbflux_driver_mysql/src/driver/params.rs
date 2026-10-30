//! Driver parameters of a MySQL/MariaDB connection, mapped onto the `mysql`
//! crate's options through `OptsBuilder::from_hash_map`, the same parser the
//! crate uses for URL query options. Validation therefore follows the crate:
//! an unknown key or a value it cannot parse fails before any socket opens.

use std::collections::HashMap;

use dbflux_core::{
    CompressionParam, ConnectionProfile, DbError, DriverParam, DriverParamKey, DriverParamsSpec,
    driver_params_map, refuse_form_owned_params,
};
use mysql::{Compression, Opts, OptsBuilder, UrlError};

pub(crate) const COMPRESSION_PARAM: CompressionParam = CompressionParam {
    key: "compress",
    on_value: "true",
    off_value: "false",
};

/// Keys the crate accepts that DBFlux passes through. `tcp_keepalive_probe_*`
/// and `tcp_user_timeout_ms` are only parsed by the crate on the platforms it
/// supports them on and are rejected as unknown elsewhere.
pub(crate) static MYSQL_DRIVER_PARAMS: DriverParamsSpec = DriverParamsSpec {
    keys: &[
        DriverParamKey {
            key: "compress",
            example: "true | false | fast | best | 0-9",
        },
        DriverParamKey {
            key: "tcp_connect_timeout_ms",
            example: "10000",
        },
        DriverParamKey {
            key: "tcp_keepalive_time_ms",
            example: "60000",
        },
        DriverParamKey {
            key: "tcp_keepalive_probe_interval_secs",
            example: "10",
        },
        DriverParamKey {
            key: "tcp_keepalive_probe_count",
            example: "3",
        },
        DriverParamKey {
            key: "tcp_user_timeout_ms",
            example: "30000",
        },
        DriverParamKey {
            key: "stmt_cache_size",
            example: "32",
        },
        DriverParamKey {
            key: "max_allowed_packet",
            example: "67108864",
        },
        DriverParamKey {
            key: "secure_auth",
            example: "true",
        },
        DriverParamKey {
            key: "enable_cleartext_plugin",
            example: "true",
        },
    ],
    compression: Some(COMPRESSION_PARAM),
};

/// Keys owned by the connection form or by the access method. `socket` and
/// `prefer_socket` are among them because a local socket would bypass the
/// configured host, tunnel or proxy.
fn is_form_owned(key: &str) -> bool {
    matches!(
        key,
        "host"
            | "port"
            | "user"
            | "password"
            | "pass"
            | "db_name"
            | "database"
            | "dbname"
            | "socket"
            | "prefer_socket"
    ) || key.starts_with("ssl")
        || key.starts_with("tls")
        || key.contains("verify")
}

/// Pool options the crate parses but that do nothing for the plain
/// connections DBFlux opens, so accepting them would only mislead.
fn is_pool_only(key: &str) -> bool {
    matches!(
        key,
        "pool_min" | "pool_max" | "reset_connection" | "check_health"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompressionMode {
    /// `compress` carries a level the crate parses.
    Explicit,
    /// `compress=false`: the crate has no "off" value, so DBFlux handles it.
    Off,
    /// No `compress` key: on for a tunneled connection, off for a direct one.
    Auto,
}

/// Validated driver parameters of one connection, ready to apply to the
/// options of every connection the driver opens for it.
#[derive(Debug, Clone)]
pub(crate) struct MysqlDriverParams {
    crate_options: HashMap<String, String>,
    compression: CompressionMode,
    tunneled: bool,
}

impl MysqlDriverParams {
    pub(crate) fn from_profile(profile: &ConnectionProfile) -> Result<Self, DbError> {
        Self::resolve(&profile.driver_params, profile.is_tunneled())
    }

    pub(crate) fn resolve(params: &[DriverParam], tunneled: bool) -> Result<Self, DbError> {
        let mut crate_options = driver_params_map(params);
        refuse_form_owned_params("MySQL", &crate_options, is_form_owned)?;

        let mut keys: Vec<&String> = crate_options.keys().collect();
        keys.sort();
        if let Some(key) = keys.into_iter().find(|key| is_pool_only(key)) {
            return Err(DbError::InvalidProfile(format!(
                "MySQL driver parameter `{key}` configures a connection pool, which DBFlux \
                 does not use; remove it from the driver parameters"
            )));
        }

        let compression = match crate_options.get(COMPRESSION_PARAM.key) {
            None => CompressionMode::Auto,
            Some(value)
                if value.eq_ignore_ascii_case(COMPRESSION_PARAM.off_value)
                    || value.eq_ignore_ascii_case("off") =>
            {
                crate_options.remove(COMPRESSION_PARAM.key);
                CompressionMode::Off
            }
            Some(_) => CompressionMode::Explicit,
        };

        let resolved = Self {
            crate_options,
            compression,
            tunneled,
        };

        // A dry run against empty options surfaces the crate's own verdict
        // on every key and value before anything connects.
        resolved.apply(OptsBuilder::new().into())?;

        Ok(resolved)
    }

    /// Applies the parameters on top of `opts`. Whatever `opts` already
    /// carries (the form fields, or the query options of a URI) is
    /// overridden by an explicit parameter.
    pub(crate) fn apply(&self, opts: Opts) -> Result<Opts, DbError> {
        let builder = OptsBuilder::from_opts(opts);
        let builder = if self.crate_options.is_empty() {
            builder
        } else {
            builder
                .from_hash_map(&self.crate_options)
                .map_err(|error| param_error(&error))?
        };

        let opts: Opts = match self.compression {
            CompressionMode::Explicit => builder.into(),
            CompressionMode::Off => builder.compress(None).into(),
            CompressionMode::Auto => {
                let opts: Opts = builder.into();
                if self.tunneled && opts.get_compress().is_none() {
                    // Compression only takes effect when the server
                    // advertises it, so asking for it cannot break a
                    // connection to a server without it.
                    OptsBuilder::from_opts(opts)
                        .compress(Some(Compression::default()))
                        .into()
                } else {
                    opts
                }
            }
        };

        Ok(opts)
    }
}

fn param_error(error: &UrlError) -> DbError {
    let message = match error {
        UrlError::UnknownParameter(key) => format!(
            "Unknown MySQL driver parameter `{key}`. Supported keys: {}",
            supported_keys()
        ),
        UrlError::InvalidValue(key, value) => {
            format!("Invalid value `{value}` for MySQL driver parameter `{key}`")
        }
        UrlError::FeatureRequired(feature, key) => {
            format!("MySQL driver parameter `{key}` needs the `{feature}` feature of the client")
        }
        other => format!("Invalid MySQL driver parameters: {other}"),
    };
    DbError::InvalidProfile(message)
}

fn supported_keys() -> String {
    MYSQL_DRIVER_PARAMS
        .keys
        .iter()
        .map(|key| key.key)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;
