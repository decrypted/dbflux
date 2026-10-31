use crate::engine::LuaRuntimeState;
use dbflux_core::{
    DetachedProcessHandle, OutputEvent, OutputStreamKind, ProcessExecutionError,
    execute_streaming_process,
};
use mlua::{Lua, Result as LuaResult, Table, Value};
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

pub(crate) fn register_logging_api(lua: &Lua, state: LuaRuntimeState) -> LuaResult<()> {
    let dbflux = ensure_dbflux_table(lua)?;
    let logging = lua.create_table()?;

    logging.set(
        "info",
        log_function(lua, state.clone(), "INFO", |message| {
            log::info!("[lua] {message}");
        })?,
    )?;
    logging.set(
        "warn",
        log_function(lua, state.clone(), "WARN", |message| {
            log::warn!("[lua] {message}");
        })?,
    )?;
    logging.set(
        "error",
        log_function(lua, state, "ERROR", |message| {
            log::error!("[lua] {message}");
        })?,
    )?;

    dbflux.set("log", logging)
}

/// IPC auth-token env-var names that must never be returned to Lua scripts, even
/// when env_read is enabled. These map to dbflux_ipc::{DRIVER_RPC_AUTH_TOKEN_ENV,
/// APP_CONTROL_AUTH_TOKEN_ENV, AUTH_PROVIDER_RPC_AUTH_TOKEN_ENV}.
const BLOCKED_ENV_VARS: &[&str] = &[
    "DBFLUX_DRIVER_IPC_TOKEN",
    "DBFLUX_IPC_TOKEN",
    "DBFLUX_AUTH_PROVIDER_IPC_TOKEN",
];

pub(crate) fn register_env_api(lua: &Lua) -> LuaResult<()> {
    let dbflux = ensure_dbflux_table(lua)?;
    let env = lua.create_table()?;

    env.set(
        "get",
        lua.create_function(|_, key: String| {
            if BLOCKED_ENV_VARS
                .iter()
                .any(|blocked| blocked.eq_ignore_ascii_case(&key))
            {
                return Ok(None);
            }
            Ok(std::env::var(key).ok())
        })?,
    )?;

    dbflux.set("env", env)
}

pub(crate) fn register_process_api(lua: &Lua, state: LuaRuntimeState) -> LuaResult<()> {
    let dbflux = ensure_dbflux_table(lua)?;
    let process = lua.create_table()?;

    process.set(
        "run",
        lua.create_function(move |lua, options: Table| run_process(lua, &state, options))?,
    )?;

    dbflux.set("process", process)
}

fn ensure_dbflux_table(lua: &Lua) -> LuaResult<Table> {
    let globals = lua.globals();

    if let Ok(existing) = globals.get::<Table>("dbflux") {
        return Ok(existing);
    }

    let dbflux = lua.create_table()?;
    globals.set("dbflux", dbflux.clone())?;
    Ok(dbflux)
}

fn log_function<F>(
    lua: &Lua,
    state: LuaRuntimeState,
    level: &'static str,
    forward: F,
) -> LuaResult<mlua::Function>
where
    F: Fn(&str) + Send + 'static,
{
    lua.create_function(move |_, message: String| {
        append_log(
            &state,
            OutputStreamKind::Log,
            format!("[{level}] {message}"),
        );
        forward(&message);
        Ok(())
    })
}

/// Parent variables a `dbflux.process.run` child keeps on every platform: enough
/// to find programs, the home directory, the locale, the time zone and a terminal.
/// Everything else, such as cloud credentials, tokens or `SSH_AUTH_SOCK`, has to be
/// passed per call through `env`, so a hook hands over only what it names.
const INHERITED_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "TERM",
    "TZ",
    // The POSIX and glibc locale categories by name: an `LC_` prefix match would
    // also pass custom `LC_*` variables, which common SSH configs forward (`SendEnv LC_*`).
    "LC_ALL",
    "LC_ADDRESS",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_IDENTIFICATION",
    "LC_MEASUREMENT",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NAME",
    "LC_NUMERIC",
    "LC_PAPER",
    "LC_TELEPHONE",
    "LC_TIME",
];

/// The user session and a desktop, so a CLI can still open a browser for an SSO
/// login (`xdg-open`, Python's `webbrowser`, an X client needing its auth cookie).
#[cfg(not(windows))]
const PLATFORM_ENV_VARS: &[&str] = &[
    "USER",
    "LOGNAME",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "XDG_CURRENT_DESKTOP",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "BROWSER",
];

/// What Windows programs and their launchers (`az.cmd`, `gcloud.cmd`) expect to
/// find: system and profile folders, program folders and the processor details.
#[cfg(windows)]
const PLATFORM_ENV_VARS: &[&str] = &[
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "OS",
    "TEMP",
    "TMP",
    "USERNAME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "ALLUSERSPROFILE",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "COMMONPROGRAMFILES",
    "COMMONPROGRAMFILES(X86)",
    "COMMONPROGRAMW6432",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
];

fn is_inherited_env_var(key: &str) -> bool {
    INHERITED_ENV_VARS
        .iter()
        .chain(PLATFORM_ENV_VARS)
        .any(|allowed| env_name_matches(allowed, key))
}

// Windows variable names are case-insensitive (`Path`, `SystemRoot`); on Unix `Home`
// is a different variable from `HOME` and is not on the list.
#[cfg(windows)]
fn env_name_matches(allowed: &str, key: &str) -> bool {
    allowed.eq_ignore_ascii_case(key)
}

#[cfg(not(windows))]
fn env_name_matches(allowed: &str, key: &str) -> bool {
    allowed == key
}

/// Builds a child's whole environment: the allowlisted part of `parent`, then the
/// script's explicit `env`, which wins on a clash.
fn child_environment(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    explicit: &[(String, String)],
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = parent
        .into_iter()
        .filter(|(key, _)| key.to_str().is_some_and(is_inherited_env_var))
        .collect();

    environment.extend(
        explicit
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value))),
    );

    environment
}

fn run_process(lua: &Lua, state: &LuaRuntimeState, options: Table) -> LuaResult<Table> {
    run_process_with_parent_env(lua, state, options, std::env::vars_os())
}

// Takes the parent environment as an argument so tests can plant variables without
// mutating the process-global environment other test threads read.
fn run_process_with_parent_env(
    lua: &Lua,
    state: &LuaRuntimeState,
    options: Table,
    parent_env: impl IntoIterator<Item = (OsString, OsString)>,
) -> LuaResult<Table> {
    let program = read_required_string(&options, "program")?;
    let allowlist = read_required_string(&options, "allowlist")?;
    let args = read_string_list(&options, "args")?;
    let timeout = read_optional_u64(&options, "timeout_ms")?.map(Duration::from_millis);
    let cwd = read_optional_string(&options, "cwd")?;
    let stream_output = read_optional_bool(&options, "stream")?.unwrap_or(false);
    let detached = read_optional_bool(&options, "detached")?.unwrap_or(false);
    let explicit_env = read_env_table(&options, "env")?;

    if state.cancel_token.is_cancelled()
        || state
            .parent_cancel_token
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    {
        return Err(mlua::Error::RuntimeError("Lua hook cancelled".to_string()));
    }

    if state
        .hook_timeout
        .is_some_and(|limit| state.hook_started_at.elapsed() >= limit)
    {
        return Err(mlua::Error::RuntimeError("Lua hook timed out".to_string()));
    }

    ensure_program_allowed(&program, &allowlist)?;

    let resolved = resolve_program_in_path(&program);

    if !detached && timeout.is_none() && state.hook_timeout.is_none() {
        return Err(mlua::Error::RuntimeError(
            "dbflux.process.run requires a timeout_ms when no hook-level timeout is set"
                .to_string(),
        ));
    }

    let exec_target = resolved
        .as_deref()
        .map(|p| p.as_os_str())
        .unwrap_or_else(|| program.as_ref());
    let mut command = Command::new(exec_target);
    command.args(&args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.env_clear();
    command.envs(child_environment(parent_env, &explicit_env));

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let resolved_display = resolved
        .as_deref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unresolved>".to_string());

    append_log(
        state,
        OutputStreamKind::Log,
        format!(
            "[PROCESS/{allowlist}] {} -> {}{}",
            program,
            resolved_display,
            if args.is_empty() {
                String::new()
            } else {
                format!(" {}", args.join(" "))
            }
        ),
    );

    let mut child = command.spawn().map_err(|error| {
        mlua::Error::RuntimeError(format!("Failed to spawn process '{program}': {error}"))
    })?;

    if detached {
        let Some(detached_sender) = &state.detached else {
            return Err(mlua::Error::RuntimeError(
                "Detached processes are not available in this context".to_string(),
            ));
        };

        let description = format_process_description(&program, &args);

        detached_sender
            .send(DetachedProcessHandle::new(
                child,
                description,
                timeout,
                None,
                None,
                None,
            ))
            .map_err(|_| {
                mlua::Error::RuntimeError("Failed to register detached process".to_string())
            })?;

        return process_result_table(lua, None, String::new(), String::new(), false, true);
    }

    let hook_timeout_remaining = state
        .hook_timeout
        .map(|limit| limit.saturating_sub(state.hook_started_at.elapsed()));

    let output = stream_output.then_some(()).and(state.output.as_ref());

    match execute_streaming_process(
        &mut child,
        // `dbflux.process.run` spawns outside the hook process group and stays out
        // of scope for job-object containment (#206).
        None,
        &state.cancel_token,
        state.parent_cancel_token.as_ref(),
        timeout,
        hook_timeout_remaining,
        output,
    ) {
        Ok(result) => process_result_table(
            lua,
            result.exit_code,
            result.stdout,
            result.stderr,
            result.timed_out,
            false,
        ),
        Err(ProcessExecutionError::Cancelled { .. }) => {
            Err(mlua::Error::RuntimeError("Lua hook cancelled".to_string()))
        }
        Err(ProcessExecutionError::TimedOut { .. }) => {
            Err(mlua::Error::RuntimeError("Lua hook timed out".to_string()))
        }
        Err(ProcessExecutionError::Spawn(error) | ProcessExecutionError::Wait(error)) => Err(
            mlua::Error::RuntimeError(format!("Failed to run process '{program}': {error}")),
        ),
    }
}

fn process_result_table(
    lua: &Lua,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    detached: bool,
) -> LuaResult<Table> {
    let result = lua.create_table()?;
    result.set("ok", detached || (exit_code == Some(0) && !timed_out))?;
    result.set("detached", detached)?;
    result.set("exit_code", exit_code)?;
    result.set("stdout", stdout)?;
    result.set("stderr", stderr)?;
    result.set("timed_out", timed_out)?;
    Ok(result)
}

fn format_process_description(program: &str, args: &[String]) -> String {
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{} {}", program, args.join(" "))
    }
}

/// Resolves a bare program name to an absolute executable path by walking `$PATH`.
///
/// First match wins. On Unix a candidate must be a file with an executable bit set;
/// on other platforms `is_file()` is sufficient. Returns `None` when `PATH` is unset
/// or no candidate matches. Best-effort only: the allowlist already gated the name —
/// this exists to make the executed binary visible in the run trail, not to enforce trust.
fn resolve_program_in_path(program: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Checks whether a program name is path-qualified (contains a path separator or leading `~`).
///
/// This is a footgun guard: it prevents accidental execution of path-qualified names like
/// `/usr/bin/aws` or `../aws` before the allowlist is checked. It is NOT a security
/// isolation boundary — PATH-order manipulation can still substitute a binary, and
/// resolving that requires `which`-style resolution which is out of scope here.
fn is_path_qualified(program: &str) -> bool {
    program.contains('/')
        || program.contains('\\')
        || Path::new(program).components().count() > 1
        || program.starts_with('~')
}

/// Validates that `program` is a bare command name on the named allowlist.
///
/// The allowlist is an ergonomics/footgun guard: it prevents typos and unintended
/// execution of programs the hook author did not expect, and it forbids path-qualified
/// names. It is NOT a security isolation boundary — a user who controls `$PATH` can
/// still substitute a different binary under the same name. To make the substitution
/// auditable, [`run_process`] resolves the name against `$PATH` and records the resolved
/// absolute path (or `<unresolved>`) in the run trail.
fn ensure_program_allowed(program: &str, allowlist: &str) -> LuaResult<()> {
    if is_path_qualified(program) {
        return Err(mlua::Error::RuntimeError(format!(
            "Program '{program}' must be a bare command name (no path separators); \
             allowlist '{allowlist}' resolves via PATH"
        )));
    }

    let Some(allowed_programs) = allowlist_programs(allowlist) else {
        return Err(mlua::Error::RuntimeError(format!(
            "dbflux.process.run allowlist '{allowlist}' is not recognized"
        )));
    };

    if allowed_programs
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(program))
    {
        Ok(())
    } else {
        Err(mlua::Error::RuntimeError(format!(
            "Program '{program}' is not allowed by allowlist '{allowlist}'"
        )))
    }
}

fn allowlist_programs(allowlist: &str) -> Option<&'static [&'static str]> {
    match allowlist {
        "aws_cli" => Some(&["aws", "aws.exe"]),
        "python_cli" => Some(&["python", "python.exe", "python3", "python3.exe"]),
        "ssh_cli" => Some(&["ssh", "ssh.exe"]),
        "cloudflared" => Some(&["cloudflared", "cloudflared.exe"]),
        "gcloud_cli" => Some(&["gcloud", "gcloud.cmd", "gcloud.exe"]),
        "az_cli" => Some(&["az", "az.cmd", "az.exe"]),
        _ => None,
    }
}

fn read_required_string(options: &Table, key: &str) -> LuaResult<String> {
    match options.get::<Value>(key)? {
        Value::String(value) => Ok(value.to_str()?.to_string()),
        Value::Nil => Err(mlua::Error::RuntimeError(format!(
            "dbflux.process.run requires '{key}'"
        ))),
        _ => Err(mlua::Error::RuntimeError(format!(
            "dbflux.process.run field '{key}' must be a string"
        ))),
    }
}

fn read_optional_string(options: &Table, key: &str) -> LuaResult<Option<String>> {
    match options.get::<Value>(key)? {
        Value::String(value) => Ok(Some(value.to_str()?.to_string())),
        Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(format!(
            "dbflux.process.run field '{key}' must be a string"
        ))),
    }
}

fn read_optional_u64(options: &Table, key: &str) -> LuaResult<Option<u64>> {
    match options.get::<Value>(key)? {
        Value::Integer(value) if value >= 0 => Ok(Some(value as u64)),
        Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(format!(
            "dbflux.process.run field '{key}' must be a non-negative integer"
        ))),
    }
}

fn read_optional_bool(options: &Table, key: &str) -> LuaResult<Option<bool>> {
    match options.get::<Value>(key)? {
        Value::Boolean(value) => Ok(Some(value)),
        Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(format!(
            "dbflux.process.run field '{key}' must be a boolean"
        ))),
    }
}

fn read_env_table(options: &Table, key: &str) -> LuaResult<Vec<(String, String)>> {
    let invalid = || {
        mlua::Error::RuntimeError(format!(
            "dbflux.process.run field '{key}' must be a table of variable names to string values"
        ))
    };

    let table = match options.get::<Value>(key)? {
        Value::Table(table) => table,
        Value::Nil => return Ok(Vec::new()),
        _ => return Err(invalid()),
    };

    let mut variables = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (Value::String(name), Value::String(value)) = pair? else {
            return Err(invalid());
        };
        let name = name.to_str()?.to_string();
        let value = value.to_str()?.to_string();

        // `Command` fails at spawn on a NUL with a vaguer message, and a name with `=`
        // would reach the child as a different variable.
        if name.is_empty() || name.contains('=') || name.contains('\0') || value.contains('\0') {
            return Err(mlua::Error::RuntimeError(format!(
                "dbflux.process.run env variable '{name}' has an invalid name or value"
            )));
        }

        variables.push((name, value));
    }

    // Lua table order is unspecified; sort so the child's environment is reproducible.
    variables.sort();
    Ok(variables)
}

fn read_string_list(options: &Table, key: &str) -> LuaResult<Vec<String>> {
    match options.get::<Value>(key)? {
        Value::Table(table) => table
            .sequence_values::<String>()
            .collect::<Result<Vec<_>, _>>(),
        Value::Nil => Ok(Vec::new()),
        _ => Err(mlua::Error::RuntimeError(format!(
            "dbflux.process.run field '{key}' must be an array of strings"
        ))),
    }
}

#[expect(
    clippy::expect_used,
    reason = "poisoned log lock is a hard failure by design: silently dropping a log entry would corrupt the run's output; the existing panic message is the intended contract"
)]
fn append_log(state: &LuaRuntimeState, stream: OutputStreamKind, message: String) {
    state
        .log_buffer
        .lock()
        .expect("lua log buffer poisoned")
        .push(message.clone());

    if let Some(output) = &state.output {
        let _ = output.send(OutputEvent::new(stream, format!("{message}\n")));
    }
}

#[cfg(test)]
mod tests;
