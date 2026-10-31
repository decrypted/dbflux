use std::sync::{Mutex, MutexGuard, PoisonError};

/// Held by every test that spawns a child process and by the test that swaps the
/// process-global `PATH`: under a threaded `cargo test`, a spawn during the swap
/// cannot find the interpreter.
static PATH_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn path_lock() -> MutexGuard<'static, ()> {
    PATH_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}
