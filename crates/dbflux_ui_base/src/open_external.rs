//! Handing a local file to the operating system's default application.
//!
//! Used by surfaces that cannot render a file themselves (PDFs, archives, and
//! any other binary payload) and instead download it and let the desktop open
//! it. The spawn is deliberately fire-and-forget: the handler process outlives
//! the call and DBFlux never waits on it.

use dbflux_core::LogErr;
use dbflux_core::local_files::{SafeFileName, create_private_file, private_dir_in};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Directory holding files downloaded only so they can be opened by an
/// external application, private to this user.
///
/// The files are server bytes the user may not want others to read, and a
/// directory with a fixed name in the shared temp dir can be created first
/// by another local user, who could then read, swap or redirect every file
/// written into it. On Linux it lives in `$XDG_RUNTIME_DIR`, which is
/// per-user and cleared at logout; elsewhere, or without one, in the temp
/// dir under a name that carries the user id.
pub fn external_open_dir() -> Result<PathBuf, String> {
    let (parent, name) = external_open_location();

    private_dir_in(&parent, &name)
        .map_err(|e| format!("Failed to create {name} in {}: {e}", parent.display()))
}

/// Reserves a new file only this user can read for `name` inside
/// [`external_open_dir`], numbered when the name is taken, so a download
/// never writes into a file or link that already exists.
pub fn create_external_open_file(name: &SafeFileName) -> Result<PathBuf, String> {
    let dir = external_open_dir()?;

    create_private_file(&dir, name)
        .map_err(|e| format!("Failed to create a file in {}: {}", dir.display(), e))
}

fn external_open_location() -> (PathBuf, String) {
    external_open_location_in(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

fn external_open_location_in(runtime_dir: Option<PathBuf>) -> (PathBuf, String) {
    #[cfg(target_os = "linux")]
    if let Some(runtime_dir) = runtime_dir
        && runtime_dir.is_absolute()
        && runtime_dir.is_dir()
    {
        return (runtime_dir, "dbflux-open".to_string());
    }
    #[cfg(not(target_os = "linux"))]
    let _no_runtime_dir_outside_linux = runtime_dir;

    #[cfg(unix)]
    {
        (
            std::env::temp_dir(),
            format!("dbflux-open-{}", dbflux_core::local_files::effective_uid()),
        )
    }

    #[cfg(not(unix))]
    {
        (std::env::temp_dir(), "dbflux-open".to_string())
    }
}

/// Opens `path` with the system's default handler for its type.
///
/// Returns `false` when the handler could not be spawned at all; a handler
/// that starts and then fails on its own is outside what the OS reports back
/// here, so a `true` return only means "the launch was accepted".
pub fn open_external(path: &Path) -> bool {
    let mut command = handler_command(path);

    command
        .spawn()
        .log_err_with("failed to open file with the system handler")
        .is_some()
}

#[cfg(target_os = "linux")]
fn handler_command(path: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(path);
    command
}

#[cfg(target_os = "macos")]
fn handler_command(path: &Path) -> Command {
    let mut command = Command::new("open");
    command.arg(path);
    command
}

#[cfg(target_os = "windows")]
fn handler_command(path: &Path) -> Command {
    // `start` is a cmd builtin, not an executable; its first quoted argument is
    // the window title, so an empty one must precede the path.
    let mut command = Command::new("cmd");
    command.arg("/C").arg("start").arg("").arg(path);
    command
}

#[cfg(test)]
mod tests {
    use super::external_open_location_in;

    /// Without a runtime dir the scratch directory sits in the shared temp
    /// dir, so its name carries the user id instead of being the fixed
    /// `dbflux-open` another local user could create first.
    #[cfg(unix)]
    #[test]
    fn the_temp_dir_fallback_is_named_per_user() {
        let (parent, name) = external_open_location_in(None);

        assert_eq!(parent, std::env::temp_dir());
        assert_eq!(
            name,
            format!("dbflux-open-{}", dbflux_core::local_files::effective_uid())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_runtime_dir_is_used_when_there_is_one() {
        let runtime_dir = std::env::temp_dir();

        let (parent, name) = external_open_location_in(Some(runtime_dir.clone()));

        assert_eq!(parent, runtime_dir);
        assert_eq!(name, "dbflux-open");
    }
}
