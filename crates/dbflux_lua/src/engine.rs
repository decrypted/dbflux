use crate::api;
use crate::api::hook::LuaHookOutcome;
use dbflux_core::{
    CancelToken, DetachedProcessSender, HookContext, HookPhase, LuaCapabilities, OutputSender,
};
use mlua::{ChunkMode, Function, Lua, LuaOptions, Result as LuaResult, StdLib, Table, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct LuaRuntimeState {
    pub outcome: Arc<Mutex<LuaHookOutcome>>,
    pub log_buffer: Arc<Mutex<Vec<String>>>,
    pub output: Option<OutputSender>,
    pub detached: Option<DetachedProcessSender>,
    pub cancel_token: CancelToken,
    pub parent_cancel_token: Option<CancelToken>,
    pub hook_started_at: Instant,
    pub hook_timeout: Option<Duration>,
}

pub(crate) struct LuaVm {
    pub lua: Lua,
    pub state: LuaRuntimeState,
}

pub(crate) struct LuaVmConfig<'a> {
    pub context: &'a HookContext,
    pub phase: HookPhase,
    pub capabilities: &'a LuaCapabilities,
    pub cancel_token: CancelToken,
    pub parent_cancel_token: Option<CancelToken>,
    pub output: Option<OutputSender>,
    pub detached: Option<DetachedProcessSender>,
    pub hook_started_at: Instant,
    pub hook_timeout: Option<Duration>,
}

/// Base-library globals mlua always opens, which reach past the capability model.
/// `load` accepts precompiled chunks, and Lua 5.4 does not verify bytecode, so a
/// crafted chunk can corrupt the interpreter's memory. `loadfile` and `dofile` read
/// any file the process can read and echo fragments of it in syntax errors.
/// `collectgarbage` lets a script stop the collector or change its tuning.
const REMOVED_BASE_GLOBALS: &[&str] = &["load", "loadfile", "dofile", "collectgarbage"];

/// `string.dump` serializes functions into bytecode; with no loader left it has no
/// use, and it is the other half of a bytecode-injection chain.
const REMOVED_STRING_FUNCTIONS: &[&str] = &["dump"];

pub(crate) struct LuaEngine;

impl LuaEngine {
    pub(crate) fn create_vm(config: LuaVmConfig<'_>) -> LuaResult<LuaVm> {
        let LuaVmConfig {
            context,
            phase,
            capabilities,
            cancel_token,
            parent_cancel_token,
            output,
            detached,
            hook_started_at,
            hook_timeout,
        } = config;

        let stdlib = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8;
        let lua = Lua::new_with(stdlib, LuaOptions::default())?;
        lua.set_memory_limit(16 * 1024 * 1024)?;
        remove_unsafe_builtins(&lua)?;

        let state = LuaRuntimeState {
            outcome: Arc::new(Mutex::new(LuaHookOutcome::Ok)),
            log_buffer: Arc::new(Mutex::new(Vec::new())),
            output,
            detached,
            cancel_token,
            parent_cancel_token,
            hook_started_at,
            hook_timeout,
        };

        api::hook::register_hook_api(&lua, phase, state.outcome.clone())?;

        if capabilities.connection_metadata {
            api::connection::register_connection_api(&lua, context)?;
        }

        if capabilities.logging {
            api::dbflux::register_logging_api(&lua, state.clone())?;
        }

        if capabilities.env_read {
            api::dbflux::register_env_api(&lua)?;
        }

        if capabilities.process_run {
            api::dbflux::register_process_api(&lua, state.clone())?;
        }

        Ok(LuaVm { lua, state })
    }
}

fn remove_unsafe_builtins(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();
    for name in REMOVED_BASE_GLOBALS {
        globals.raw_set(*name, Value::Nil)?;
    }

    // The string metatable's `__index` is this same table, so `(""):dump` goes too.
    let string: Table = globals.raw_get("string")?;
    for name in REMOVED_STRING_FUNCTIONS {
        string.raw_set(*name, Value::Nil)?;
    }

    Ok(())
}

impl LuaVm {
    /// Compiles a hook script as source text only. Left to auto-detection, mlua
    /// treats input starting with the bytecode signature as a binary chunk.
    #[track_caller]
    pub(crate) fn load_script(&self, source: &[u8]) -> LuaResult<Function> {
        self.lua
            .load(source)
            .set_mode(ChunkMode::Text)
            .into_function()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbflux_core::{CancelToken, HookPhase};
    use uuid::Uuid;

    fn test_context() -> HookContext {
        HookContext {
            profile_id: Uuid::nil(),
            profile_name: "lua-engine-test".to_string(),
            db_kind: "Postgres".to_string(),
            host: Some("localhost".to_string()),
            port: Some(5432),
            database: Some("dbflux".to_string()),
            phase: Some(HookPhase::PostConnect),
            dials_configured_endpoint: false,
        }
    }

    fn test_vm_config<'a>(
        context: &'a HookContext,
        phase: HookPhase,
        capabilities: &'a LuaCapabilities,
    ) -> LuaVmConfig<'a> {
        LuaVmConfig {
            context,
            phase,
            capabilities,
            cancel_token: CancelToken::new(),
            parent_cancel_token: None,
            output: None,
            detached: None,
            hook_started_at: Instant::now(),
            hook_timeout: None,
        }
    }

    #[test]
    fn registers_hook_phase_and_connection_metadata() {
        let context = test_context();
        let capabilities = LuaCapabilities::default();
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PostConnect,
            &capabilities,
        ))
        .unwrap();

        let phase: String = vm.lua.load("return hook.phase").eval().unwrap();
        let profile_name: String = vm
            .lua
            .load("return connection.profile_name")
            .eval()
            .unwrap();
        let host: String = vm.lua.load("return connection.host").eval().unwrap();

        assert_eq!(phase, "post_connect");
        assert_eq!(profile_name, "lua-engine-test");
        assert_eq!(host, "localhost");
    }

    #[test]
    fn does_not_load_unsafe_libraries() {
        let context = test_context();
        let capabilities = LuaCapabilities::default();
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();

        let io_is_nil: bool = vm.lua.load("return io == nil").eval().unwrap();
        let os_is_nil: bool = vm.lua.load("return os == nil").eval().unwrap();
        let debug_is_nil: bool = vm.lua.load("return debug == nil").eval().unwrap();
        let package_is_nil: bool = vm.lua.load("return package == nil").eval().unwrap();

        assert!(io_is_nil);
        assert!(os_is_nil);
        assert!(debug_is_nil);
        assert!(package_is_nil);
    }

    #[test]
    fn loaders_and_collector_control_are_removed() {
        let context = test_context();
        let capabilities = LuaCapabilities::all_enabled();
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();

        for expression in [
            "load",
            "loadfile",
            "dofile",
            "collectgarbage",
            "string.dump",
            "(\"\").dump",
            "getmetatable(\"\").__index.dump",
            "require",
            "package",
        ] {
            let is_nil: bool = vm
                .lua
                .load(format!("return {expression} == nil"))
                .eval()
                .unwrap();
            assert!(is_nil, "{expression} must be nil in the hook sandbox");
        }

        for call in [
            "load('return 1')",
            "loadfile('/etc/hostname')",
            "dofile('/etc/hostname')",
            "collectgarbage('stop')",
            "string.dump(function() end)",
        ] {
            let result = vm.lua.load(call).exec();
            assert!(result.is_err(), "{call} must fail in the hook sandbox");
        }
    }

    #[test]
    fn hook_script_bytecode_is_refused() {
        let context = test_context();
        let capabilities = LuaCapabilities::default();
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();
        // A separate, unrestricted VM stands in for whoever precompiled the chunk.
        let bytecode = Lua::new()
            .load("return 42")
            .into_function()
            .unwrap()
            .dump(false);

        let error = vm.load_script(&bytecode).unwrap_err().to_string();

        assert!(
            error.contains("binary chunk"),
            "bytecode must be refused, got: {error}"
        );
        let answer: i64 = vm.load_script(b"return 42").unwrap().call(()).unwrap();
        assert_eq!(answer, 42, "source text still loads");
    }

    /// Every global and standard-library member a hook can reach, with every
    /// capability on. A new entry here is a sandbox decision: anything that reads
    /// files, loads code, runs processes or reaches the network belongs behind a
    /// capability, not in this list.
    const SANDBOX_GLOBALS: &[&str] = &[
        "_G",
        "_VERSION",
        "assert",
        "connection",
        "dbflux",
        "error",
        "getmetatable",
        "hook",
        "ipairs",
        "math",
        "next",
        "pairs",
        "pcall",
        "print",
        "rawequal",
        "rawget",
        "rawlen",
        "rawset",
        "select",
        "setmetatable",
        "string",
        "table",
        "tonumber",
        "tostring",
        "type",
        "utf8",
        "warn",
        "xpcall",
    ];

    const SANDBOX_LIBRARY_MEMBERS: &[(&str, &[&str])] = &[
        (
            "string",
            &[
                "byte", "char", "find", "format", "gmatch", "gsub", "len", "lower", "match",
                "pack", "packsize", "rep", "reverse", "sub", "unpack", "upper",
            ],
        ),
        (
            "table",
            &[
                "concat", "insert", "move", "pack", "remove", "sort", "unpack",
            ],
        ),
        (
            "math",
            // mlua's vendored Lua keeps the 5.3 compatibility functions (atan2, cosh,
            // frexp, ldexp, log10, pow, sinh, tanh).
            &[
                "abs",
                "acos",
                "asin",
                "atan",
                "atan2",
                "ceil",
                "cos",
                "cosh",
                "deg",
                "exp",
                "floor",
                "fmod",
                "frexp",
                "huge",
                "ldexp",
                "log",
                "log10",
                "max",
                "maxinteger",
                "min",
                "mininteger",
                "modf",
                "pi",
                "pow",
                "rad",
                "random",
                "randomseed",
                "sin",
                "sinh",
                "sqrt",
                "tan",
                "tanh",
                "tointeger",
                "type",
                "ult",
            ],
        ),
        (
            "utf8",
            &["char", "charpattern", "codepoint", "codes", "len", "offset"],
        ),
        ("dbflux", &["env", "log", "process"]),
    ];

    fn sorted_keys(table: &Table) -> Vec<String> {
        let mut keys: Vec<String> = table
            .pairs::<Value, Value>()
            .map(|pair| {
                let (key, _) = pair.unwrap();
                match key {
                    Value::String(name) => name.to_str().unwrap().to_string(),
                    other => format!("<non-string key {other:?}>"),
                }
            })
            .collect();
        keys.sort();
        keys
    }

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut names: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        names.sort();
        names
    }

    #[test]
    fn sandbox_globals_match_the_allowlist() {
        let context = test_context();
        let capabilities = LuaCapabilities::all_enabled();
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();
        let globals = vm.lua.globals();

        assert_eq!(
            sorted_keys(&globals),
            sorted(SANDBOX_GLOBALS),
            "sandbox globals changed; review the new entry before extending SANDBOX_GLOBALS"
        );

        for (library, members) in SANDBOX_LIBRARY_MEMBERS {
            let table: Table = globals.get(*library).unwrap();
            assert_eq!(
                sorted_keys(&table),
                sorted(members),
                "members of `{library}` changed; review them before extending the allowlist"
            );
        }
    }

    #[test]
    fn capabilities_hide_optional_apis() {
        let context = test_context();
        let capabilities = LuaCapabilities {
            logging: false,
            env_read: false,
            connection_metadata: false,
            process_run: false,
        };
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();

        let connection_is_nil: bool = vm.lua.load("return connection == nil").eval().unwrap();
        let logging_is_nil: bool = vm
            .lua
            .load("return dbflux == nil or dbflux.log == nil")
            .eval()
            .unwrap();
        let env_is_nil: bool = vm
            .lua
            .load("return dbflux == nil or dbflux.env == nil")
            .eval()
            .unwrap();

        assert!(connection_is_nil);
        assert!(logging_is_nil);
        assert!(env_is_nil);
    }

    #[test]
    fn logging_and_env_api_are_available_when_enabled() {
        let context = test_context();
        let capabilities = LuaCapabilities {
            env_read: true,
            ..LuaCapabilities::default()
        };
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();

        let has_logging: bool = vm
            .lua
            .load("return dbflux ~= nil and dbflux.log ~= nil")
            .eval()
            .unwrap();
        let has_env: bool = vm
            .lua
            .load("return dbflux ~= nil and dbflux.env ~= nil")
            .eval()
            .unwrap();
        let path_value: Option<String> =
            vm.lua.load("return dbflux.env.get('PATH')").eval().unwrap();

        assert!(has_logging);
        assert!(has_env);
        assert!(path_value.is_some());
    }

    #[test]
    fn env_api_is_hidden_by_default() {
        let context = test_context();
        let capabilities = LuaCapabilities::default();
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();

        let env_is_nil: bool = vm
            .lua
            .load("return dbflux == nil or dbflux.env == nil")
            .eval()
            .unwrap();

        assert!(
            env_is_nil,
            "env_read defaults to false; dbflux.env must be nil"
        );
    }

    #[test]
    fn process_api_is_hidden_when_capability_is_disabled() {
        let context = test_context();
        let capabilities = LuaCapabilities::default();
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();

        let has_process: bool = vm
            .lua
            .load("return dbflux == nil or dbflux.process ~= nil")
            .eval()
            .unwrap();

        assert!(!has_process);
    }

    #[test]
    fn process_api_is_available_when_capability_is_enabled() {
        let context = test_context();
        let capabilities = LuaCapabilities {
            process_run: true,
            ..LuaCapabilities::default()
        };
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();

        let has_process: bool = vm
            .lua
            .load("return dbflux ~= nil and dbflux.process ~= nil and dbflux.process.run ~= nil")
            .eval()
            .unwrap();

        assert!(has_process);
    }

    #[test]
    fn exceeding_memory_limit_returns_error() {
        let context = test_context();
        let capabilities = LuaCapabilities::default();
        let vm = LuaEngine::create_vm(test_vm_config(
            &context,
            HookPhase::PreConnect,
            &capabilities,
        ))
        .unwrap();

        let result: mlua::Result<()> = vm
            .lua
            .load(
                r#"
            local t = {}
            for i = 1, 10000000 do
                t[i] = string.rep("x", 1024)
            end
            "#,
            )
            .exec();

        assert!(result.is_err(), "Script exceeding memory limit should fail");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not enough memory"),
            "Error should mention memory: {}",
            err_msg
        );
    }
}
