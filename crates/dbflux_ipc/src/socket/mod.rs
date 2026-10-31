//! Local IPC endpoints for the app-control channel and the RPC service hosts.
//!
//! On Unix every socket is a filesystem socket inside a directory only this
//! user can enter. The Linux abstract namespace and the `/tmp` fallback that
//! `interprocess` uses for "namespaced" names elsewhere have no owner, so any
//! local user could bind a name first and receive the auth token and the
//! connection secrets a client sends to what it believes is its own service.
//! This file is the only place that may build a namespaced name
//! (`scripts/lint.py abstract-sockets` enforces it): Windows named pipes, and
//! the old name for tests that stand in for a squatter.

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{
    Listener, ListenerNonblockingMode, ListenerOptions, Name, Stream, prelude::*,
};

/// Directory a managed host is told to bind in, so it uses the one the parent
/// resolved even when its own environment would pick another.
pub const IPC_DIR_ENV: &str = "DBFLUX_IPC_DIR";

const RUNTIME_SUBDIR: &str = "dbflux";
const CACHE_SUBDIR: &str = "dbflux-ipc";

/// Longest socket path `bind` accepts, without the trailing NUL: `sun_path`
/// is 108 bytes on Linux and 104 on macOS and the BSDs.
#[cfg(unix)]
const MAX_SOCKET_PATH_BYTES: usize = if cfg!(any(target_os = "linux", target_os = "android")) {
    107
} else {
    103
};

static TEST_DIR: OnceLock<PathBuf> = OnceLock::new();

fn build_suffix() -> &'static str {
    // Debug and release builds use distinct names so both can run at once.
    if cfg!(debug_assertions) { "-debug" } else { "" }
}

/// The app-control socket of the GUI (`Focus`, `OpenScript`, `Ping`).
pub fn app_control_socket() -> io::Result<IpcSocket> {
    IpcSocket::named(format!("dbflux{}.sock", build_suffix()))
}

/// The socket of a driver host addressed by its service id.
pub fn driver_socket(id: &str) -> io::Result<IpcSocket> {
    IpcSocket::named(format!("dbflux-driver-{id}{}.sock", build_suffix()))
}

/// The socket of an RPC driver service: the configured `socket_id` is the
/// socket file name as-is, which is what `dbflux-driver-host --socket` binds.
pub fn rpc_driver_socket(socket_id: &str) -> io::Result<IpcSocket> {
    IpcSocket::named(socket_id.to_string())
}

pub fn auth_provider_socket(id: &str) -> io::Result<IpcSocket> {
    IpcSocket::named(format!("dbflux-auth-provider-{id}{}.sock", build_suffix()))
}

pub fn secret_provider_socket(id: &str) -> io::Result<IpcSocket> {
    IpcSocket::named(format!(
        "dbflux-secret-provider-{id}{}.sock",
        build_suffix()
    ))
}

/// A local socket endpoint: a socket file in [`ipc_dir`] on Unix, a named
/// pipe on Windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpcSocket {
    leaf: String,
    #[cfg(unix)]
    path: PathBuf,
}

impl IpcSocket {
    fn named(leaf: String) -> io::Result<Self> {
        check_leaf(&leaf)?;

        #[cfg(unix)]
        {
            Self::in_dir(&ipc_dir()?, leaf)
        }

        #[cfg(not(unix))]
        {
            Ok(Self { leaf })
        }
    }

    #[cfg(unix)]
    fn in_dir(dir: &Path, leaf: String) -> io::Result<Self> {
        check_leaf(&leaf)?;
        let path = dir.join(&leaf);

        let length = path.as_os_str().len();
        if length > MAX_SOCKET_PATH_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "socket path {} is {length} bytes, the limit is {MAX_SOCKET_PATH_BYTES}; \
                     use a shorter socket ID or set {IPC_DIR_ENV} to a shorter directory",
                    path.display()
                ),
            ));
        }

        Ok(Self { leaf, path })
    }

    /// The socket file. Only Unix has one.
    #[cfg(unix)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> io::Result<Name<'_>> {
        #[cfg(unix)]
        {
            self.path.as_path().to_fs_name::<GenericFilePath>()
        }

        #[cfg(not(unix))]
        {
            self.leaf.as_str().to_ns_name::<GenericNamespaced>()
        }
    }

    /// Connects and refuses a server that runs as another user, before the
    /// caller sends anything. Use this for every connection that carries a
    /// token or a secret.
    pub fn connect(&self) -> io::Result<Stream> {
        self.connect_expecting_peer(own_uid())
    }

    /// [`connect`](Self::connect) with the expected server uid passed in, so a
    /// test can simulate a server that belongs to someone else.
    #[doc(hidden)]
    pub fn connect_expecting_peer(&self, expected_uid: u32) -> io::Result<Stream> {
        let stream = Stream::connect(self.name()?)?;
        verify_peer_as(&stream, expected_uid)?;
        Ok(stream)
    }

    /// Whether something accepts connections here. Sends nothing.
    pub fn is_listening(&self) -> bool {
        self.name().and_then(Stream::connect).is_ok()
    }

    /// Suffix for a "socket not available" error, naming the likely cause when
    /// a host predates the private directory. Static on purpose: probing the
    /// old abstract name would connect to a name anyone can bind, and a full
    /// backlog there blocks `connect` with no timeout.
    pub fn legacy_hint(&self) -> String {
        #[cfg(unix)]
        {
            format!(
                "; a host built for an older DBFlux may still listen on the old socket name '{}' \
                 and must be updated to listen on {self}",
                self.leaf
            )
        }

        #[cfg(not(unix))]
        {
            String::new()
        }
    }

    /// Binds a listener, replacing a stale socket file of ours but never a
    /// live one or a file that is not our socket.
    pub fn listen(&self, nonblocking: ListenerNonblockingMode) -> io::Result<Listener> {
        #[cfg(unix)]
        self.remove_stale_socket()?;

        let options = ListenerOptions::new()
            .name(self.name()?)
            .nonblocking(nonblocking)
            .reclaim_name(true)
            // Overwriting would unlink the socket of a running instance.
            .try_overwrite(false);

        // Defence in depth only: the directory already keeps other users out.
        // Other Unices ignore a mode set before bind and report Unsupported.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let options = {
            use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
            options.mode(0o600)
        };

        options.create_sync()
    }

    #[cfg(unix)]
    fn remove_stale_socket(&self) -> io::Result<()> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};

        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };

        if !metadata.file_type().is_socket() || metadata.uid() != own_uid() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "refusing to replace {}: it is not a socket of this user",
                    self.path.display()
                ),
            ));
        }

        if self.is_listening() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("{} is in use by a running process", self.path.display()),
            ));
        }

        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl fmt::Display for IpcSocket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(unix)]
        {
            write!(formatter, "{}", self.path.display())
        }

        #[cfg(not(unix))]
        {
            write!(formatter, r"\\.\pipe\{}", self.leaf)
        }
    }
}

fn check_leaf(leaf: &str) -> io::Result<()> {
    if leaf.is_empty() || leaf == "." || leaf == ".." || leaf.contains(['/', '\\', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid socket name {leaf:?}"),
        ));
    }

    Ok(())
}

/// Refuses a peer that does not run as this user. Servers call it on every
/// accepted connection before reading a request.
pub fn verify_peer(stream: &Stream) -> io::Result<()> {
    verify_peer_as(stream, own_uid())
}

/// [`verify_peer`] with the expected uid passed in, for tests. On Windows no
/// peer check is made yet: pipes have no uid, the default pipe DACL is not
/// owner-only, and checking the pipe server's user is still to be done.
#[doc(hidden)]
pub fn verify_peer_as(stream: &Stream, expected_uid: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        check_peer_uid(stream.peer_creds()?.euid(), expected_uid)
    }

    #[cfg(not(unix))]
    {
        let _ = (stream, expected_uid);
        Ok(())
    }
}

#[cfg(unix)]
fn check_peer_uid(peer_uid: Option<u32>, expected_uid: u32) -> io::Result<()> {
    match peer_uid {
        Some(uid) if uid == expected_uid => Ok(()),
        Some(uid) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("local socket peer runs as uid {uid}, expected {expected_uid}"),
        )),
        None => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local socket peer credentials are unavailable",
        )),
    }
}

/// The uid peers must run as: the effective uid on Unix, 0 elsewhere.
#[cfg(unix)]
pub fn own_uid() -> u32 {
    dbflux_core::local_files::effective_uid()
}

/// The uid peers must run as: the effective uid on Unix, 0 elsewhere.
#[cfg(not(unix))]
pub fn own_uid() -> u32 {
    0
}

/// The private directory that holds the sockets: `$DBFLUX_IPC_DIR` (a plain
/// absolute path whose parent exists; an empty value counts as unset), else
/// `$XDG_RUNTIME_DIR/dbflux` (`/run/user/<uid>/dbflux` on Linux when unset),
/// else `dbflux-ipc` in the user cache directory.
/// Created `0700` when missing; an existing one is used only when it is a
/// real directory owned by this user that nobody else can write.
pub fn ipc_dir() -> io::Result<PathBuf> {
    // This crate's own unit tests never resolve the real directory.
    if cfg!(test) {
        return isolate_ipc_dir_for_tests();
    }

    if let Some(dir) = TEST_DIR.get() {
        return Ok(dir.clone());
    }

    resolve_ipc_dir(&DirSources::from_env(), own_uid())
}

/// Tells a managed host where to bind, so it lands in the directory this
/// process connects to even if its own environment differs.
pub fn pass_ipc_dir_to(command: &mut std::process::Command) -> io::Result<()> {
    #[cfg(unix)]
    command.env(IPC_DIR_ENV, ipc_dir()?);

    #[cfg(not(unix))]
    let _ = command;

    Ok(())
}

struct DirSources {
    explicit: Option<PathBuf>,
    runtime: Option<PathBuf>,
    cache: Option<PathBuf>,
}

impl DirSources {
    fn from_env() -> Self {
        let absolute_env = |key: &str| {
            std::env::var_os(key)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };

        // A process started outside the desktop session, such as `dbflux mcp`
        // launched by an AI client, often lacks XDG_RUNTIME_DIR; the systemd
        // default keeps it on the same directory as the GUI and the hosts.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let session_default = Some(PathBuf::from(format!("/run/user/{}", own_uid())));
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let session_default = None;

        Self {
            explicit: std::env::var_os(IPC_DIR_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            runtime: absolute_env("XDG_RUNTIME_DIR").or(session_default),
            cache: dirs::cache_dir(),
        }
    }
}

fn resolve_ipc_dir(sources: &DirSources, owner: u32) -> io::Result<PathBuf> {
    if let Some(explicit) = &sources.explicit {
        return explicit_ipc_dir(explicit, owner);
    }

    if let Some(runtime) = sources
        .runtime
        .as_deref()
        .filter(|runtime| runtime_dir_is_usable(runtime, owner))
    {
        return private_dir(runtime, RUNTIME_SUBDIR, owner);
    }

    let Some(cache) = &sources.cache else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_RUNTIME_DIR nor a cache directory is available for IPC sockets",
        ));
    };

    std::fs::create_dir_all(cache)?;
    private_dir(cache, CACHE_SUBDIR, owner)
}

/// `$DBFLUX_IPC_DIR`, exactly as given: a hand-started host binds
/// `$DBFLUX_IPC_DIR/<name>`, so any renaming would put the two ends in
/// different places. Only a plain absolute path is accepted: with a trailing
/// `/`, `.` or `..` the final lstat resolves the last component, so a symlink
/// there would pass the checks meant for the entry itself.
fn explicit_ipc_dir(explicit: &Path, owner: u32) -> io::Result<PathBuf> {
    let normalized: PathBuf = explicit.components().collect();
    let plain = explicit.is_absolute()
        && normalized.as_os_str() == explicit.as_os_str()
        && !explicit
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir));

    let (true, Some(parent), Some(_)) = (plain, explicit.parent(), explicit.file_name()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{IPC_DIR_ENV}={} must be an absolute directory path without a trailing `/`, \
                 `.` or `..`",
                explicit.display()
            ),
        ));
    };

    let with_context = |error: io::Error, path: &Path| {
        io::Error::new(
            error.kind(),
            format!(
                "{IPC_DIR_ENV}={}: {}: {error}",
                explicit.display(),
                path.display()
            ),
        )
    };

    check_ancestors(parent, owner, with_context)?;

    dbflux_core::local_files::private_dir_at_owned_by(explicit, owner).map_err(not_private)
}

/// Every directory above the IPC directory must be one nobody else can change,
/// or another user could rename the IPC directory away after the checks and
/// put their own in its place. Links above it are allowed (`/tmp` is one on
/// macOS): the given path is walked entry by entry, so a link's own owner is
/// checked, and then the resolved path, so its target is checked too. The
/// path DBFlux uses stays the one configured.
#[cfg(unix)]
fn check_ancestors(
    parent: &Path,
    owner: u32,
    with_context: impl Fn(io::Error, &Path) -> io::Error,
) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let canonical = parent
        .canonicalize()
        .map_err(|error| with_context(error, parent))?;

    let given = parent
        .ancestors()
        .map(|dir| (dir, std::fs::symlink_metadata(dir)));
    let resolved = canonical
        .ancestors()
        .map(|dir| (dir, std::fs::metadata(dir)));

    for (dir, metadata) in given.chain(resolved) {
        let metadata = metadata.map_err(|error| with_context(error, dir))?;
        let is_link = metadata.file_type().is_symlink();

        if !ancestor_is_trusted(
            metadata.uid(),
            metadata.gid(),
            metadata.mode(),
            is_link,
            owner,
        ) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "IPC socket directory is not private: {} above it can be changed by \
                     another user",
                    dir.display()
                ),
            ));
        }
    }

    Ok(())
}

#[cfg(not(unix))]
fn check_ancestors(
    _parent: &Path,
    _owner: u32,
    _with_context: impl Fn(io::Error, &Path) -> io::Error,
) -> io::Result<()> {
    Ok(())
}

/// Whether an entry above the IPC directory is out of other users' reach. It
/// must belong to `owner` or root. A link's mode means nothing; only who owns
/// it. A directory must not be writable by others unless sticky, so they
/// cannot rename what they do not own. Group write is accepted only on a
/// directory of `owner` whose group id equals `owner`: the per-user group that
/// common Linux distributions create, where umask 002 makes a user's own
/// folders group-writable for that user alone. A shared group (a team group,
/// `staff` on macOS, a root-owned group-writable directory) is refused, since
/// its other members could swap the entry.
#[cfg(unix)]
fn ancestor_is_trusted(uid: u32, gid: u32, mode: u32, is_link: bool, owner: u32) -> bool {
    if uid != owner && uid != 0 {
        return false;
    }

    if is_link {
        return true;
    }

    let sticky = mode & 0o1000 != 0;
    let world_writable = mode & 0o002 != 0;
    let group_writable = mode & 0o020 != 0;
    let private_group = uid == owner && gid == owner;

    sticky || !(world_writable || (group_writable && !private_group))
}

/// The XDG spec requires the runtime directory to belong to the user; one
/// that does not (a stale value after `su`, say) is skipped, not trusted.
fn runtime_dir_is_usable(path: &Path, owner: u32) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.is_dir() && metadata.uid() == owner
    }

    #[cfg(not(unix))]
    {
        let _ = owner;
        metadata.is_dir()
    }
}

fn private_dir(parent: &Path, leaf: &str, owner: u32) -> io::Result<PathBuf> {
    dbflux_core::local_files::private_dir_in_owned_by(parent, leaf, owner).map_err(not_private)
}

fn not_private(error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("IPC socket directory is not private: {error}"),
    )
}

/// Points every socket of this process at a fresh private directory under the
/// system temp dir, so tests never touch the sockets of a running DBFlux.
/// Idempotent within a process; returns the directory.
#[doc(hidden)]
pub fn isolate_ipc_dir_for_tests() -> io::Result<PathBuf> {
    if let Some(dir) = TEST_DIR.get() {
        return Ok(dir.clone());
    }

    // Short on purpose: the whole socket path must fit in `sun_path`.
    let leaf = format!("dbx-ipc-{:08x}", uuid::Uuid::new_v4().as_u128() as u32);
    let dir = dbflux_core::local_files::private_dir_in(&std::env::temp_dir(), &leaf)?;

    Ok(TEST_DIR.get_or_init(|| dir).clone())
}

/// Binds the pre-fix name of `socket` (the abstract name on Linux), standing
/// in for another user who squats it.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[doc(hidden)]
pub fn bind_legacy_name_for_tests(socket: &IpcSocket) -> io::Result<Listener> {
    ListenerOptions::new()
        .name(socket.leaf.as_str().to_ns_name::<GenericNamespaced>()?)
        .nonblocking(ListenerNonblockingMode::Accept)
        .create_sync()
}

#[cfg(test)]
#[cfg(unix)]
mod tests;
