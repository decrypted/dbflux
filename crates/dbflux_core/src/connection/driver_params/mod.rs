//! Per-connection driver parameters: free-form `key = value` pairs a driver
//! maps onto its client library's connection options (DataGrip's "Advanced"
//! tab).
//!
//! The parameters live on [`ConnectionProfile`](crate::ConnectionProfile),
//! outside the driver config, so rewriting host and port for a tunnel or proxy
//! (`DbConfig::redirect_to_tunnel`) keeps them. Values are plain text: they
//! never pass through value sources or the keyring, so they must not hold
//! secrets.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::DbError;

/// One `key = value` pair of a connection's driver parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverParam {
    pub key: String,
    pub value: String,
}

impl DriverParam {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Folds an ordered parameter list into a map. A later entry for the same key
/// replaces an earlier one, and entries with a blank key are dropped, so the
/// list the user edits never has to be deduplicated first.
pub fn driver_params_map(params: &[DriverParam]) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(params.len());
    for param in params {
        let key = param.key.trim();
        if key.is_empty() {
            continue;
        }
        map.insert(key.to_string(), param.value.trim().to_string());
    }
    map
}

/// The value of `key` in an ordered parameter list, with the same last-wins
/// rule as [`driver_params_map`].
pub fn driver_param_value<'a>(params: &'a [DriverParam], key: &str) -> Option<&'a str> {
    params
        .iter()
        .rev()
        .find(|param| param.key.trim() == key)
        .map(|param| param.value.trim())
}

/// Rejects the first parameter whose key names a setting the connection form
/// owns (host, credentials, TLS, ...). Matching ignores ASCII case so a
/// `Host` or `sslMode` copied from another client is caught too.
pub fn refuse_form_owned_params(
    driver_name: &str,
    params: &HashMap<String, String>,
    is_form_owned: impl Fn(&str) -> bool,
) -> Result<(), DbError> {
    let mut keys: Vec<&String> = params.keys().collect();
    keys.sort();

    for key in keys {
        if is_form_owned(&key.to_ascii_lowercase()) {
            return Err(DbError::InvalidProfile(format!(
                "{driver_name} driver parameter `{key}` is set by the connection form \
                 (host, port, user, password, database, SSL and access method fields); \
                 remove it from the driver parameters"
            )));
        }
    }

    Ok(())
}

/// What the connection manager offers for a driver's parameters. Returned by
/// `DbDriver::driver_params_spec`; a driver without one has no parameters
/// section.
#[derive(Debug, Clone, Copy)]
pub struct DriverParamsSpec {
    /// Keys the driver accepts, shown as hints in the editor. The driver's own
    /// validation stays authoritative.
    pub keys: &'static [DriverParamKey],
    /// A compression switch drawn in the Transport section that edits one
    /// parameter of the list.
    pub compression: Option<CompressionParam>,
}

/// A parameter key a driver documents, with an example value.
#[derive(Debug, Clone, Copy)]
pub struct DriverParamKey {
    pub key: &'static str,
    pub example: &'static str,
}

/// A three-way compression switch backed by one driver parameter, so the
/// parameter list stays the single source of truth: `Auto` is the absence of
/// the key, `On` and `Off` write `on_value` and `off_value`.
#[derive(Debug, Clone, Copy)]
pub struct CompressionParam {
    pub key: &'static str,
    pub on_value: &'static str,
    pub off_value: &'static str,
}

/// The state of a [`CompressionParam`] switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionChoice {
    /// No parameter: the driver decides (compressed through a tunnel, proxy
    /// or SSM, uncompressed for direct connections).
    Auto,
    On,
    Off,
}

impl CompressionChoice {
    pub const ALL: [CompressionChoice; 3] = [
        CompressionChoice::Auto,
        CompressionChoice::On,
        CompressionChoice::Off,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CompressionChoice::Auto => "auto",
            CompressionChoice::On => "on",
            CompressionChoice::Off => "off",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.as_str() == id)
    }
}

impl CompressionParam {
    /// The switch position the parameter list describes. Any value other
    /// than `off_value` counts as on, so a level such as `best` written by
    /// hand still shows as on.
    pub fn choice(&self, params: &[DriverParam]) -> CompressionChoice {
        match driver_param_value(params, self.key) {
            None => CompressionChoice::Auto,
            Some(value) if value.eq_ignore_ascii_case(self.off_value) => CompressionChoice::Off,
            Some(_) => CompressionChoice::On,
        }
    }

    /// Writes `choice` into the parameter list. The first entry for the key
    /// keeps its position and later duplicates are dropped, so the list and
    /// the switch cannot disagree afterwards.
    pub fn apply(&self, params: &mut Vec<DriverParam>, choice: CompressionChoice) {
        let value = match choice {
            CompressionChoice::Auto => None,
            CompressionChoice::On => Some(self.on_value),
            CompressionChoice::Off => Some(self.off_value),
        };

        if choice == CompressionChoice::On && self.choice(params) == CompressionChoice::On {
            return;
        }

        let mut kept_first = false;
        params.retain_mut(|param| {
            if param.key.trim() != self.key {
                return true;
            }
            match value {
                Some(value) if !kept_first => {
                    kept_first = true;
                    param.key = self.key.to_string();
                    param.value = value.to_string();
                    true
                }
                _ => false,
            }
        });

        if let Some(value) = value
            && !kept_first
        {
            params.push(DriverParam::new(self.key, value));
        }
    }
}

#[cfg(test)]
mod tests;
