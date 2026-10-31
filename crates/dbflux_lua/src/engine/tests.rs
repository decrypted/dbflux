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
            "byte", "char", "find", "format", "gmatch", "gsub", "len", "lower", "match", "pack",
            "packsize", "rep", "reverse", "sub", "unpack", "upper",
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
    let path_value: Option<String> = vm.lua.load("return dbflux.env.get('PATH')").eval().unwrap();

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
