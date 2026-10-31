use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use super::file_name::{SafeFileName, confined_join};

/// Numbered names tried before [`create_private_file`] gives up, so a
/// directory full of earlier copies cannot keep it looping.
const MAX_NAME_ATTEMPTS: u32 = 10_000;

/// `parent/name` as a directory only this user can use, created when it is
/// missing.
///
/// On Unix an existing entry is used only when it is a real directory, not
/// a symbolic link, owned by this user and not writable by anyone else: in a
/// shared directory such as `/tmp` another user can create the name first
/// and then read, replace or redirect every file written into it. A
/// directory of ours that others can only read is narrowed to `0700`.
pub fn private_dir_in(parent: &Path, name: &str) -> io::Result<PathBuf> {
    #[cfg(unix)]
    let owner = effective_uid();
    #[cfg(not(unix))]
    let owner = 0;

    private_dir_in_owned_by(parent, name, owner)
}

/// [`private_dir_in`] with the expected owner passed in, so a test can make a
/// directory of its own look like another user's. `owner` is ignored outside
/// Unix.
#[doc(hidden)]
pub fn private_dir_in_owned_by(parent: &Path, name: &str, owner: u32) -> io::Result<PathBuf> {
    let path = confined_join(parent, &SafeFileName::new(name))?;
    ensure_private_dir(&path, owner)?;
    Ok(path)
}

/// Creates a new, empty file only this user can read and write, named
/// `name` in `directory` or the first [`SafeFileName::numbered`] form of it
/// that is free, and returns its path.
///
/// The name is reserved with `create_new`, which fails on any existing
/// entry, a dangling symbolic link included, so no existing file is opened
/// or truncated and no link is followed.
pub fn create_private_file(directory: &Path, name: &SafeFileName) -> io::Result<PathBuf> {
    for attempt in 1..=MAX_NAME_ATTEMPTS {
        let candidate = if attempt == 1 {
            name.clone()
        } else {
            name.numbered(attempt)
        };
        let path = confined_join(directory, &candidate)?;

        match private_new_file_options().open(&path) {
            Ok(file) => {
                drop(file);
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "no free name for {} in {}",
            name.as_str(),
            directory.display()
        ),
    ))
}

fn private_new_file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }

    options
}

#[cfg(unix)]
fn ensure_private_dir(path: &Path, owner: u32) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(refused(path, "is not a directory"));
    }
    if metadata.uid() != owner {
        return Err(refused(path, "belongs to another user"));
    }

    let mode = metadata.mode() & 0o777;
    if mode & 0o022 != 0 {
        return Err(refused(path, "can be written by other users"));
    }
    if mode != 0o700 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }

    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_dir(path: &Path, _owner: u32) -> io::Result<()> {
    std::fs::create_dir_all(path)?;

    if std::fs::symlink_metadata(path)?.file_type().is_dir() {
        Ok(())
    } else {
        Err(refused(path, "is not a directory"))
    }
}

fn refused(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("refusing to use {}: it {reason}", path.display()),
    )
}

#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "libc FFI: geteuid takes no arguments, cannot fail and reads no memory"
)]
/// The effective user id of this process.
pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and only returns a value.
    unsafe { libc::geteuid() }
}
