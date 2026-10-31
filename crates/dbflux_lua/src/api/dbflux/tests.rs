use super::*;
use crate::api::hook::LuaHookOutcome;
use crate::test_support::path_lock;
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn test_state(
    hook_timeout: Option<Duration>,
    hook_started_at: Instant,
    output: Option<dbflux_core::OutputSender>,
) -> LuaRuntimeState {
    LuaRuntimeState {
        outcome: Arc::new(Mutex::new(LuaHookOutcome::Ok)),
        log_buffer: Arc::new(Mutex::new(Vec::new())),
        output,
        detached: None,
        cancel_token: dbflux_core::CancelToken::new(),
        parent_cancel_token: None,
        hook_started_at,
        hook_timeout,
    }
}

fn python_program() -> &'static str {
    if cfg!(target_os = "windows") {
        "python"
    } else {
        "python3"
    }
}

fn process_options(lua: &Lua, program: &str) -> LuaResult<Table> {
    let options = lua.create_table()?;
    options.set("program", program)?;
    options.set("allowlist", "python_cli")?;
    Ok(options)
}

#[test]
fn run_process_rejects_expired_hook_timeout_before_spawn() {
    let lua = Lua::new();
    let options = process_options(&lua, python_program()).unwrap();
    let state = test_state(
        Some(Duration::from_millis(1)),
        Instant::now() - Duration::from_secs(1),
        None,
    );

    let error = run_process(&lua, &state, options).unwrap_err().to_string();

    assert!(error.contains("timed out"));
}

#[test]
fn append_log_keeps_buffer_plain_but_streams_newline() {
    let (sender, receiver) = dbflux_core::output_channel();
    let state = test_state(None, Instant::now(), Some(sender));

    append_log(&state, OutputStreamKind::Log, "hello-log".to_string());

    let buffered = state.log_buffer.lock().unwrap().clone();
    let event = receiver.try_recv().unwrap();

    assert_eq!(buffered, vec!["hello-log"]);
    assert_eq!(event.stream, OutputStreamKind::Log);
    assert_eq!(event.text, "hello-log\n");
}

// Streaming of a child's stdout/stderr is covered directly in dbflux_core by
// `process_executor_emits_streaming_output_events`, `execute_captures_stderr`, and
// `execute_streaming_process_emits_partial_line_output_before_newline`. A
// run_process-level variant added nothing but a real `python3` spawn, which is
// flaky under CI process pressure, so it is intentionally not duplicated here.
#[test]
fn run_process_cancellation_returns_cancelled_error() {
    let lua = Lua::new();
    let state = test_state(None, Instant::now(), None);
    let options = process_options(&lua, python_program()).unwrap();
    let args = lua.create_table().unwrap();

    args.set(1, "-c").unwrap();
    args.set(2, "import time; time.sleep(30)").unwrap();
    options.set("args", args).unwrap();
    options.set("stream", true).unwrap();
    options.set("timeout_ms", 30000_i64).unwrap();

    // An already-cancelled token must surface as a cancellation error before the
    // child is spawned. Cancelling mid-flight instead would race process startup
    // under load; the streaming monitor's mid-execution cancellation is covered by
    // the hook executor's own tests.
    state.cancel_token.cancel();

    let error = run_process(&lua, &state, options).unwrap_err().to_string();

    assert!(error.contains("cancelled"));
}

// =========================================================================
// Path-separator rejection
// =========================================================================

#[test]
fn test_absolute_path_rejected() {
    let err = ensure_program_allowed("/usr/bin/aws", "aws_cli").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("bare command name") || msg.contains("path separator"),
        "expected path-separator error, got: {msg}"
    );
}

#[test]
fn test_relative_path_rejected() {
    let err = ensure_program_allowed("../aws", "aws_cli").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("bare command name") || msg.contains("path separator"),
        "expected path-separator error, got: {msg}"
    );
}

#[test]
fn test_bare_allowlisted_passes() {
    assert!(ensure_program_allowed("aws", "aws_cli").is_ok());
}

#[test]
fn test_bare_non_allowlisted_fails_at_allowlist() {
    let err = ensure_program_allowed("malicious_tool", "aws_cli").unwrap_err();
    let msg = err.to_string();
    assert!(
        !msg.contains("bare command name") && !msg.contains("path separator"),
        "expected allowlist error (not path-separator), got: {msg}"
    );
}

// =========================================================================
// Timeout required for non-detached process.run
// =========================================================================

#[test]
fn test_no_timeout_and_no_hook_deadline_rejected() {
    let lua = Lua::new();
    let options = lua.create_table().unwrap();
    options.set("program", "aws").unwrap();
    options.set("allowlist", "aws_cli").unwrap();

    let state = test_state(None, Instant::now(), None);

    let err = run_process(&lua, &state, options).unwrap_err().to_string();
    assert!(
        err.contains("requires a timeout_ms"),
        "expected timeout-required error, got: {err}"
    );
}

#[test]
fn test_timeout_ms_set_accepted() {
    let lua = Lua::new();
    let options = lua.create_table().unwrap();
    options.set("program", "aws").unwrap();
    options.set("allowlist", "aws_cli").unwrap();
    options.set("timeout_ms", 5000_i64).unwrap();

    let state = test_state(None, Instant::now(), None);

    let result = run_process(&lua, &state, options);
    if let Err(e) = result {
        let msg = e.to_string();
        assert!(
            !msg.contains("requires a timeout_ms"),
            "unexpected timeout-required error: {msg}"
        );
    }
}

#[test]
fn test_hook_deadline_present_accepted() {
    let lua = Lua::new();
    let options = lua.create_table().unwrap();
    options.set("program", "aws").unwrap();
    options.set("allowlist", "aws_cli").unwrap();

    let state = test_state(Some(Duration::from_secs(60)), Instant::now(), None);

    let result = run_process(&lua, &state, options);
    if let Err(e) = result {
        let msg = e.to_string();
        assert!(
            !msg.contains("requires a timeout_ms"),
            "unexpected timeout-required error when hook timeout is set: {msg}"
        );
    }
}

#[test]
fn test_detached_exempt() {
    let lua = Lua::new();
    let options = lua.create_table().unwrap();
    options.set("program", "aws").unwrap();
    options.set("allowlist", "aws_cli").unwrap();
    options.set("detached", true).unwrap();

    let state = test_state(None, Instant::now(), None);

    let result = run_process(&lua, &state, options);
    if let Err(e) = result {
        let msg = e.to_string();
        assert!(
            !msg.contains("requires a timeout_ms"),
            "detached run must not require timeout_ms, got: {msg}"
        );
    }
}

// =========================================================================
// IPC token blocklist
// =========================================================================

fn make_env_lua() -> Lua {
    let lua = Lua::new();
    register_env_api(&lua).expect("register_env_api failed");
    lua
}

#[expect(
    unsafe_code,
    reason = "pre-existing fixture mutates process-global env via set_var/remove_var (unsafe since edition 2024); scoped to this test, no safety claim; fixture hardening tracked separately and not fixed here"
)]
#[test]
fn env_get_blocks_driver_ipc_token_exact_case() {
    // Set the var so it would be readable if not blocked.
    unsafe { std::env::set_var("DBFLUX_DRIVER_IPC_TOKEN", "should-be-blocked") };
    let lua = make_env_lua();
    let value: Option<String> = lua
        .load("return dbflux.env.get('DBFLUX_DRIVER_IPC_TOKEN')")
        .eval()
        .unwrap();
    unsafe { std::env::remove_var("DBFLUX_DRIVER_IPC_TOKEN") };
    assert!(
        value.is_none(),
        "DBFLUX_DRIVER_IPC_TOKEN must be blocked even when env_read is enabled"
    );
}

#[expect(
    unsafe_code,
    reason = "pre-existing fixture mutates process-global env via set_var/remove_var (unsafe since edition 2024); scoped to this test, no safety claim; fixture hardening tracked separately and not fixed here"
)]
#[test]
fn env_get_blocks_ipc_token_case_insensitive() {
    unsafe { std::env::set_var("DBFLUX_IPC_TOKEN", "should-be-blocked") };
    let lua = make_env_lua();
    let lower: Option<String> = lua
        .load("return dbflux.env.get('dbflux_ipc_token')")
        .eval()
        .unwrap();
    let upper: Option<String> = lua
        .load("return dbflux.env.get('DBFLUX_IPC_TOKEN')")
        .eval()
        .unwrap();
    unsafe { std::env::remove_var("DBFLUX_IPC_TOKEN") };
    assert!(lower.is_none(), "lowercase key must be blocked");
    assert!(upper.is_none(), "uppercase key must be blocked");
}

#[expect(
    unsafe_code,
    reason = "pre-existing fixture mutates process-global env via set_var/remove_var (unsafe since edition 2024); scoped to this test, no safety claim; fixture hardening tracked separately and not fixed here"
)]
#[test]
fn env_get_blocks_auth_provider_ipc_token() {
    unsafe { std::env::set_var("DBFLUX_AUTH_PROVIDER_IPC_TOKEN", "should-be-blocked") };
    let lua = make_env_lua();
    let value: Option<String> = lua
        .load("return dbflux.env.get('DBFLUX_AUTH_PROVIDER_IPC_TOKEN')")
        .eval()
        .unwrap();
    unsafe { std::env::remove_var("DBFLUX_AUTH_PROVIDER_IPC_TOKEN") };
    assert!(
        value.is_none(),
        "DBFLUX_AUTH_PROVIDER_IPC_TOKEN must be blocked"
    );
}

#[expect(
    unsafe_code,
    reason = "pre-existing fixture mutates process-global env via set_var/remove_var (unsafe since edition 2024); scoped to this test, no safety claim; fixture hardening tracked separately and not fixed here"
)]
#[test]
fn env_get_allows_unrelated_vars() {
    unsafe { std::env::set_var("DBFLUX_TEST_SAFE_VAR_12345", "visible") };
    let lua = make_env_lua();
    let value: Option<String> = lua
        .load("return dbflux.env.get('DBFLUX_TEST_SAFE_VAR_12345')")
        .eval()
        .unwrap();
    unsafe { std::env::remove_var("DBFLUX_TEST_SAFE_VAR_12345") };
    assert_eq!(value.as_deref(), Some("visible"));
}

// =========================================================================
// Child environment of dbflux.process.run
// =========================================================================

const PRINT_ENV_SCRIPT: &str = "import os, sys\nfor name in sys.argv[1:]:\n    print(name + '=' + os.environ.get(name, '<unset>'))";

/// Options for a child that prints `NAME=value` (or `NAME=<unset>`) per name.
fn print_env_options(lua: &Lua, names: &[&str]) -> Table {
    let options = process_options(lua, python_program()).unwrap();
    let args = lua.create_table().unwrap();
    args.push("-c").unwrap();
    args.push(PRINT_ENV_SCRIPT).unwrap();
    for name in names {
        args.push(*name).unwrap();
    }
    options.set("args", args).unwrap();
    options.set("timeout_ms", 30_000_i64).unwrap();
    options
}

fn child_env_report(result: &Table) -> std::collections::HashMap<String, String> {
    let stdout: String = result.get("stdout").unwrap();
    let stderr: String = result.get("stderr").unwrap();
    assert!(
        result.get::<bool>("ok").unwrap(),
        "env probe child failed: stdout={stdout:?} stderr={stderr:?}"
    );
    stdout
        .lines()
        .filter_map(|line| line.trim_end().split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn planted_parent_env(extra: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .chain(
            extra
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        )
        .collect()
}

#[test]
fn process_run_child_does_not_inherit_unlisted_parent_variables() {
    let _path = path_lock();
    // Cargo and nextest both export CARGO_MANIFEST_DIR to the test process, so it
    // is a real parent variable that is not on the allowlist, read without
    // mutating the process environment.
    assert!(
        std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
        "test precondition: the runner exports CARGO_MANIFEST_DIR"
    );
    let lua = Lua::new();
    let state = test_state(None, Instant::now(), None);
    let options = print_env_options(&lua, &["CARGO_MANIFEST_DIR"]);

    let result = run_process(&lua, &state, options).unwrap();
    let report = child_env_report(&result);

    assert_eq!(
        report.get("CARGO_MANIFEST_DIR").map(String::as_str),
        Some("<unset>")
    );
}

#[test]
fn process_run_child_does_not_see_parent_secrets() {
    let _path = path_lock();
    let lua = Lua::new();
    let state = test_state(None, Instant::now(), None);
    let options = print_env_options(
        &lua,
        &[
            "AWS_SECRET_ACCESS_KEY",
            "SSH_AUTH_SOCK",
            "DBFLUX_IPC_TOKEN",
            "DBFLUX_LUA_CANARY",
            "LC_DBFLUX_TOKEN",
            "LC_MEASUREMENT",
        ],
    );
    let parent = planted_parent_env(&[
        ("AWS_SECRET_ACCESS_KEY", "lua-canary-aws"),
        ("SSH_AUTH_SOCK", "/tmp/lua-canary.sock"),
        ("DBFLUX_IPC_TOKEN", "lua-canary-ipc"),
        ("DBFLUX_LUA_CANARY", "lua-canary"),
        ("LC_DBFLUX_TOKEN", "lua-canary-lc"),
        ("LC_MEASUREMENT", "C"),
    ]);

    let result = run_process_with_parent_env(&lua, &state, options, parent).unwrap();
    let report = child_env_report(&result);

    for name in [
        "AWS_SECRET_ACCESS_KEY",
        "SSH_AUTH_SOCK",
        "DBFLUX_IPC_TOKEN",
        "DBFLUX_LUA_CANARY",
        "LC_DBFLUX_TOKEN",
    ] {
        assert_eq!(
            report.get(name).map(String::as_str),
            Some("<unset>"),
            "{name} must not reach the child"
        );
    }
    assert_eq!(
        report.get("LC_MEASUREMENT").map(String::as_str),
        Some("C"),
        "locale categories stay"
    );
}

#[test]
fn process_run_passes_explicit_env_to_child() {
    let _path = path_lock();
    let lua = Lua::new();
    let state = test_state(None, Instant::now(), None);
    let options = print_env_options(&lua, &["DBFLUX_LUA_EXPLICIT", "TZ"]);
    let env = lua.create_table().unwrap();
    env.set("DBFLUX_LUA_EXPLICIT", "from-script").unwrap();
    env.set("TZ", "Europe/Berlin").unwrap();
    options.set("env", env).unwrap();
    let parent = planted_parent_env(&[("TZ", "UTC")]);

    let result = run_process_with_parent_env(&lua, &state, options, parent).unwrap();
    let report = child_env_report(&result);

    assert_eq!(
        report.get("DBFLUX_LUA_EXPLICIT").map(String::as_str),
        Some("from-script")
    );
    assert_eq!(
        report.get("TZ").map(String::as_str),
        Some("Europe/Berlin"),
        "explicit env wins over an inherited variable"
    );
}

fn kept_names(parent: &[(&str, &str)]) -> Vec<String> {
    let parent = parent
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)));
    child_environment(parent, &[])
        .into_iter()
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect()
}

const SECRET_LIKE_PARENT_VARS: &[(&str, &str)] = &[
    ("AWS_ACCESS_KEY_ID", "x"),
    ("AWS_SESSION_TOKEN", "x"),
    ("AWS_PROFILE", "x"),
    ("GITHUB_TOKEN", "x"),
    ("SSH_AUTH_SOCK", "x"),
    ("DATABASE_URL", "x"),
    ("HTTPS_PROXY", "x"),
    ("DBFLUX_DRIVER_IPC_TOKEN", "x"),
    ("LC_DEPLOY_TOKEN", "x"),
    ("LC_TERMINAL", "x"),
    ("PATHOLOGICAL", "x"),
];

#[cfg(not(windows))]
#[test]
fn child_environment_keeps_only_allowlisted_parent_variables() {
    let mut parent = vec![
        ("PATH", "/usr/bin"),
        ("HOME", "/home/u"),
        ("Home", "/elsewhere"),
        ("LANG", "C.UTF-8"),
        ("LC_ALL", "C"),
        ("LC_TIME", "C"),
        ("TZ", "Europe/Berlin"),
        ("TMPDIR", "/tmp"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ("DISPLAY", ":0"),
        ("XAUTHORITY", "/run/user/1000/gdm/Xauthority"),
        ("BROWSER", "firefox"),
        ("SYSTEMROOT", "x"),
    ];
    parent.extend_from_slice(SECRET_LIKE_PARENT_VARS);

    assert_eq!(
        kept_names(&parent),
        [
            "PATH",
            "HOME",
            "LANG",
            "LC_ALL",
            "LC_TIME",
            "TZ",
            "TMPDIR",
            "XDG_RUNTIME_DIR",
            "DISPLAY",
            "XAUTHORITY",
            "BROWSER",
        ]
    );
}

#[cfg(windows)]
#[test]
fn child_environment_keeps_only_allowlisted_parent_variables() {
    let mut parent = vec![
        ("Path", "C:\\Windows"),
        ("SystemRoot", "C:\\Windows"),
        ("ProgramFiles(x86)", "C:\\Program Files (x86)"),
        ("PROCESSOR_ARCHITECTURE", "AMD64"),
        ("TZ", "UTC"),
        ("XAUTHORITY", "x"),
    ];
    parent.extend_from_slice(SECRET_LIKE_PARENT_VARS);

    assert_eq!(
        kept_names(&parent),
        [
            "Path",
            "SystemRoot",
            "ProgramFiles(x86)",
            "PROCESSOR_ARCHITECTURE",
            "TZ",
        ]
    );
}

#[test]
fn env_option_rejects_non_string_values_and_bad_names() {
    let lua = Lua::new();

    let options = lua.create_table().unwrap();
    options.set("env", "PATH=/bin").unwrap();
    assert!(read_env_table(&options, "env").is_err());

    let env = lua.create_table().unwrap();
    env.set("PORT", 5432_i64).unwrap();
    options.set("env", env).unwrap();
    assert!(read_env_table(&options, "env").is_err());

    let env = lua.create_table().unwrap();
    env.set("A=B", "x").unwrap();
    options.set("env", env).unwrap();
    assert!(read_env_table(&options, "env").is_err());

    let env = lua.create_table().unwrap();
    env.set("", "x").unwrap();
    options.set("env", env).unwrap();
    assert!(read_env_table(&options, "env").is_err());
}

// --- SEC2-3: PATH resolution helper ---
// All PATH-mutating sub-cases are in one test fn to avoid cross-test races.

#[expect(
    unsafe_code,
    reason = "pre-existing fixture mutates process-global PATH via set_var/remove_var (unsafe since edition 2024); covers all 5 unsafe blocks in this test fn, no safety claim; fixture hardening tracked separately and not fixed here"
)]
#[test]
fn resolve_program_in_path_cases() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp_dir(label: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "dbflux_lua_path_test_{}_{}_{}",
            std::process::id(),
            n,
            label
        ))
    }

    let _path = path_lock();
    let saved_path = std::env::var_os("PATH");

    // Sub-case: returns None when program is absent
    {
        let empty = tmp_dir("empty");
        std::fs::create_dir_all(&empty).unwrap();
        unsafe { std::env::set_var("PATH", std::env::join_paths([&empty]).unwrap()) };

        let result = resolve_program_in_path("definitely-not-here-xyz");
        assert!(result.is_none(), "absent program must return None");

        let _ = std::fs::remove_dir_all(&empty);
    }

    // Unix-only sub-cases: exec-bit detection
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Sub-case: finds a bare name when on PATH with exec bit
        {
            let dir = tmp_dir("exec");
            std::fs::create_dir_all(&dir).unwrap();
            let prog = dir.join("fakeprog");
            std::fs::write(&prog, b"#!/bin/sh").unwrap();
            let mut perms = std::fs::metadata(&prog).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&prog, perms).unwrap();

            unsafe { std::env::set_var("PATH", std::env::join_paths([&dir]).unwrap()) };
            let result = resolve_program_in_path("fakeprog");
            assert_eq!(
                result.as_deref(),
                Some(prog.as_path()),
                "exec binary must be found"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        // Sub-case: skips non-executable file (no exec bit)
        {
            let dir = tmp_dir("noexec");
            std::fs::create_dir_all(&dir).unwrap();
            let prog = dir.join("fakeprog");
            std::fs::write(&prog, b"#!/bin/sh").unwrap();
            let mut perms = std::fs::metadata(&prog).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&prog, perms).unwrap();

            unsafe { std::env::set_var("PATH", std::env::join_paths([&dir]).unwrap()) };
            let result = resolve_program_in_path("fakeprog");
            assert!(result.is_none(), "non-executable file must not be returned");

            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // Restore PATH
    match saved_path {
        Some(p) => unsafe { std::env::set_var("PATH", p) },
        None => unsafe { std::env::remove_var("PATH") },
    }
}
