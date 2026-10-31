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
mod tests;
