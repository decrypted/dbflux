//! What an imported bundle may restore without the user's explicit consent.
//!
//! A bundle is a file anyone can write by hand, and the app's own exporter
//! never sets most of what a bundle can carry. Plain connection settings are
//! taken as they are, but three kinds of entries are not:
//!
//! - hooks run programs on this machine when the connection connects or
//!   disconnects;
//! - value sources other than literals read environment variables, dotenv
//!   files or cloud secret stores on this machine and send the value to the
//!   host the bundle names;
//! - some driver parameters weaken how the password travels (for example
//!   MySQL's `enable_cleartext_plugin`).
//!
//! [`review`] lists them so a caller can show every one, and
//! [`crate::import::apply`] drops them unless [`ImportConsent`] keeps them.
//! The gate sits in `apply`, not in the UI, so an import path that shows no
//! review at all still imports none of them.

use std::collections::HashMap;

use dbflux_core::values::ValueRef;
use dbflux_core::{
    ConnectionHook, ConnectionHooks, DriverParam, HookKind, HookPhase, ScriptLanguage, ScriptSource,
};
use uuid::Uuid;

use crate::ParsedBundle;
use crate::bundle::ConnectionEntry;

/// What the user agreed to keep from the bundle. The default keeps nothing,
/// so a caller that never asks imports none of the entries this module gates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportConsent {
    /// Keep the connections' hooks.
    pub hooks: bool,
    /// Keep non-literal value sources and the driver parameters that
    /// [`driver_param_needs_consent`] flags.
    pub external_values: bool,
}

impl ImportConsent {
    /// Consent to everything a bundle can carry.
    pub fn all() -> Self {
        Self {
            hooks: true,
            external_values: true,
        }
    }
}

/// Where a value reference reads its value from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSourceKind {
    Literal,
    Env,
    Dotenv,
    Secret,
    Parameter,
    Auth,
}

/// One value reference of an imported connection. `detail` names the source
/// (variable name, file and key, secret locator); the value itself is never
/// resolved for the review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueRefReview {
    pub field: String,
    pub source: ValueSourceKind,
    pub detail: String,
}

impl ValueRefReview {
    pub fn needs_consent(&self) -> bool {
        self.source != ValueSourceKind::Literal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookReviewKind {
    Command,
    Script(ScriptLanguage),
    Lua,
}

/// One hook of an imported connection, with everything that decides what
/// it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReview {
    pub phase: HookPhase,
    pub kind: HookReviewKind,
    /// The program and its arguments for a command, the interpreter and
    /// script path (or `<inline script>`) for a script, `None` for Lua.
    pub command_line: Option<String>,
    /// The start of an inline script or Lua chunk, or the Lua file path.
    pub excerpt: Option<String>,
    pub cwd: Option<String>,
    /// Names only: the values of hook environment variables travel in the
    /// bundle's secrets section.
    pub env_keys: Vec<String>,
    pub enabled: bool,
    /// Lua capabilities that reach outside the hook (`process.run`, reading
    /// the environment).
    pub lua_process_run: bool,
    pub lua_env_read: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverParamReview {
    pub key: String,
    pub value: String,
    pub needs_consent: bool,
}

/// The gated entries of one bundle connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionReview {
    pub local_id: String,
    pub name: String,
    /// The `host` and `port` fields, so the user sees where value sources
    /// would send what they read.
    pub host: Option<String>,
    pub port: Option<String>,
    pub hooks: Vec<HookReview>,
    pub value_refs: Vec<ValueRefReview>,
    pub driver_params: Vec<DriverParamReview>,
}

impl ConnectionReview {
    fn is_empty(&self) -> bool {
        self.hooks.is_empty() && self.value_refs.is_empty() && self.driver_params.is_empty()
    }
}

/// Every hook, value reference and driver parameter `apply` would restore
/// with full consent, per connection. Connections with none are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReview {
    pub connections: Vec<ConnectionReview>,
}

impl ImportReview {
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    pub fn has_hooks(&self) -> bool {
        self.connections.iter().any(|c| !c.hooks.is_empty())
    }

    /// Whether any value reference or driver parameter needs
    /// [`ImportConsent::external_values`].
    pub fn has_external_values(&self) -> bool {
        self.connections.iter().any(|c| {
            c.value_refs.iter().any(ValueRefReview::needs_consent)
                || c.driver_params.iter().any(|p| p.needs_consent)
        })
    }
}

/// What `apply` left out of one imported connection for lack of consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldEntries {
    pub connection_id: Uuid,
    pub connection_name: String,
    pub hooks: usize,
    pub value_refs: usize,
    pub driver_params: usize,
}

/// Lists what [`crate::import::apply`] would restore with full consent. It
/// reads the bundle through the same helpers as `apply`, so a hook or value
/// reference `apply` would restore cannot be missing here.
pub fn review(parsed: &ParsedBundle) -> ImportReview {
    let secrets = parsed.decrypted_secrets.as_ref();

    let connections = parsed
        .bundle
        .connections
        .iter()
        .map(|entry| review_connection(entry, secrets))
        .filter(|review| !review.is_empty())
        .collect();

    ImportReview { connections }
}

fn review_connection(
    entry: &ConnectionEntry,
    secrets: Option<&HashMap<String, String>>,
) -> ConnectionReview {
    let hooks = crate::import::bundle_hooks(entry, secrets)
        .map(|hooks| review_hooks(&hooks))
        .unwrap_or_default();

    let mut value_refs: Vec<ValueRefReview> = crate::import::restore_value_refs(entry)
        .into_iter()
        .map(|(field, vref)| review_value_ref(field, &vref))
        .collect();
    value_refs.sort_by(|a, b| a.field.cmp(&b.field));

    let driver_params = entry
        .driver_params
        .iter()
        .map(|param| DriverParamReview {
            key: visible(&param.key),
            value: visible(&param.value),
            needs_consent: driver_param_needs_consent(param),
        })
        .collect();

    ConnectionReview {
        local_id: entry.local_id.clone(),
        name: visible(&entry.name),
        host: entry.fields.get("host").map(|host| visible(host)),
        port: entry.fields.get("port").map(|port| visible(port)),
        hooks,
        value_refs,
        driver_params,
    }
}

const PHASES: [HookPhase; 4] = [
    HookPhase::PreConnect,
    HookPhase::PostConnect,
    HookPhase::PreDisconnect,
    HookPhase::PostDisconnect,
];

fn review_hooks(hooks: &ConnectionHooks) -> Vec<HookReview> {
    PHASES
        .iter()
        .flat_map(|&phase| {
            hooks
                .phase_hooks(phase)
                .iter()
                .map(move |hook| review_hook(phase, hook))
        })
        .collect()
}

fn review_hook(phase: HookPhase, hook: &ConnectionHook) -> HookReview {
    let mut env_keys: Vec<String> = hook.env.keys().map(|key| visible(key)).collect();
    env_keys.sort();

    let (kind, command_line, excerpt, lua_process_run, lua_env_read) = match &hook.kind {
        HookKind::Command { command, args } => {
            let line = std::iter::once(command.as_str())
                .chain(args.iter().map(String::as_str))
                .map(shell_quote)
                .collect::<Vec<_>>()
                .join(" ");
            (HookReviewKind::Command, Some(line), None, false, false)
        }
        HookKind::Script {
            language,
            source,
            interpreter,
        } => {
            // An interpreter override replaces bash/python3 with any program,
            // so it is part of what the hook runs.
            let program = interpreter
                .clone()
                .or_else(|| language.default_interpreter().map(str::to_string))
                .unwrap_or_default();
            let (line, excerpt) = match source {
                ScriptSource::File { path } => (
                    format!(
                        "{} {}",
                        shell_quote(&program),
                        shell_quote(&path.display().to_string())
                    ),
                    None,
                ),
                ScriptSource::Inline { content } => (
                    format!("{} <inline script>", shell_quote(&program)),
                    Some(script_excerpt(content)),
                ),
            };
            (
                HookReviewKind::Script(*language),
                Some(line),
                excerpt,
                false,
                false,
            )
        }
        HookKind::Lua {
            source,
            capabilities,
        } => {
            let excerpt = match source {
                ScriptSource::File { path } => visible(&path.display().to_string()),
                ScriptSource::Inline { content } => script_excerpt(content),
            };
            (
                HookReviewKind::Lua,
                None,
                Some(excerpt),
                capabilities.process_run,
                capabilities.env_read,
            )
        }
    };

    HookReview {
        phase,
        kind,
        command_line: command_line.map(|line| visible(&line)),
        excerpt,
        cwd: hook
            .cwd
            .as_ref()
            .map(|cwd| visible(&cwd.display().to_string())),
        env_keys,
        enabled: hook.enabled,
        lua_process_run,
        lua_env_read,
    }
}

fn review_value_ref(field: String, vref: &ValueRef) -> ValueRefReview {
    fn with_json_key(base: String, json_key: Option<&String>) -> String {
        match json_key {
            Some(key) => format!("{base}#{key}"),
            None => base,
        }
    }

    let (source, detail) = match vref {
        // The literal itself is not shown: it may be a password, and it
        // reads nothing from this machine.
        ValueRef::Literal { .. } => (ValueSourceKind::Literal, String::new()),
        ValueRef::Env { key } => (ValueSourceKind::Env, key.clone()),
        ValueRef::Dotenv { path, key } => (ValueSourceKind::Dotenv, format!("{path}#{key}")),
        ValueRef::Secret {
            provider,
            locator,
            json_key,
        } => (
            ValueSourceKind::Secret,
            with_json_key(format!("{provider}: {locator}"), json_key.as_ref()),
        ),
        ValueRef::Parameter {
            provider,
            name,
            json_key,
        } => (
            ValueSourceKind::Parameter,
            with_json_key(format!("{provider}: {name}"), json_key.as_ref()),
        ),
        ValueRef::Auth { field } => (ValueSourceKind::Auth, field.clone()),
    };

    ValueRefReview {
        field: visible(&field),
        source,
        detail: visible(&detail),
    }
}

/// Driver parameters a bundle may set without consent, with the values that
/// keep them harmless (`None`: any value). Everything else, including keys a
/// driver gains later, needs consent: an allowlist fails safe, a denylist of
/// risky keys would not. The importer has no driver registry, so the list is
/// keyed by parameter name across drivers; the names here mean the same
/// harmless thing wherever they occur.
const PLAIN_DRIVER_PARAMS: &[(&str, Option<&[&str]>)] = &[
    // MySQL / MariaDB
    ("compress", None),
    ("tcp_connect_timeout_ms", None),
    ("tcp_keepalive_time_ms", None),
    ("tcp_keepalive_probe_interval_secs", None),
    ("tcp_keepalive_probe_count", None),
    ("tcp_user_timeout_ms", None),
    ("stmt_cache_size", None),
    ("max_allowed_packet", None),
    ("secure_auth", Some(&["true"])),
    ("enable_cleartext_plugin", Some(&["false"])),
    // PostgreSQL. `options` is left out: it sets arbitrary server settings.
    ("application_name", None),
    ("connect_timeout", None),
    ("tcp_user_timeout", None),
    ("keepalives", None),
    ("keepalives_idle", None),
    ("keepalives_interval", None),
    ("keepalives_retries", None),
    ("target_session_attrs", None),
    ("channel_binding", Some(&["prefer", "require"])),
];

/// Whether a bundle's driver parameter may only be imported with
/// [`ImportConsent::external_values`]: anything that can weaken
/// authentication or transport security, and any key not known to be
/// harmless.
pub fn driver_param_needs_consent(param: &DriverParam) -> bool {
    let key = param.key.trim();
    let value = param.value.trim();

    // A blank key is dropped by every driver, so it carries nothing.
    if key.is_empty() {
        return false;
    }

    match PLAIN_DRIVER_PARAMS.iter().find(|(plain, _)| *plain == key) {
        None => true,
        Some((_, None)) => false,
        Some((_, Some(safe_values))) => !safe_values
            .iter()
            .any(|safe| safe.eq_ignore_ascii_case(value)),
    }
}

/// Applies `consent` to one restored connection: returns what was removed.
pub(crate) fn gate_connection(
    profile: &mut dbflux_core::ConnectionProfile,
    consent: ImportConsent,
) -> Option<WithheldEntries> {
    let mut withheld = WithheldEntries {
        connection_id: profile.id,
        connection_name: profile.name.clone(),
        hooks: 0,
        value_refs: 0,
        driver_params: 0,
    };

    if !consent.hooks
        && let Some(hooks) = profile.hooks.take()
    {
        withheld.hooks = PHASES
            .iter()
            .map(|&phase| hooks.phase_hooks(phase).len())
            .sum();
    }

    if !consent.external_values {
        let before = profile.value_refs.len();
        profile
            .value_refs
            .retain(|_, vref| !vref.needs_resolution());
        withheld.value_refs = before - profile.value_refs.len();

        let before = profile.driver_params.len();
        profile
            .driver_params
            .retain(|param| !driver_param_needs_consent(param));
        withheld.driver_params = before - profile.driver_params.len();
    }

    let any = withheld.hooks + withheld.value_refs + withheld.driver_params > 0;
    any.then_some(withheld)
}

/// Shell-style quoting for display, so every argument reads as one:
/// `sh -c 'curl x | sh'` is a single script argument, not a pipeline.
fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+~".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

const EXCERPT_MAX_LINES: usize = 8;
const EXCERPT_MAX_CHARS: usize = 480;

/// Bundle text as the review shows it: control characters and the Unicode
/// direction overrides are escaped, so a name or command cannot fake extra
/// review lines or display in an order other than the one it runs in.
fn visible(text: &str) -> String {
    visible_lines(text, false)
}

fn visible_lines(text: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let hidden = c.is_control()
            || matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
        if keep_newlines && (c == '\n' || c == '\t') {
            out.push(c);
        } else if hidden {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}

/// The first lines of a script, enough to judge what it does without
/// letting a huge inline payload flood the review.
fn script_excerpt(content: &str) -> String {
    let content = visible_lines(content, true);
    let mut excerpt = String::new();
    let mut truncated = false;

    for (index, line) in content.lines().enumerate() {
        if index == EXCERPT_MAX_LINES {
            truncated = true;
            break;
        }
        if index > 0 {
            excerpt.push('\n');
        }
        excerpt.push_str(line);
    }

    if excerpt.chars().count() > EXCERPT_MAX_CHARS {
        excerpt = excerpt.chars().take(EXCERPT_MAX_CHARS).collect();
        truncated = true;
    }

    if truncated {
        excerpt.push_str("\n…");
    }

    excerpt
}

#[cfg(test)]
mod tests;
