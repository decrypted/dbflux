//! Local IPC endpoints for the app-control channel and the RPC service hosts.
//!
//! On Unix every socket is a filesystem socket inside a directory only this
//! user can enter. The Linux abstract namespace and the `/tmp` fallback that
//! `interprocess` uses for "namespaced" names elsewhere have no owner, so any
//! local user could bind a name first and receive the auth token and the
//! connection secrets a client sends to what it believes is its own service.
//! This file is the only place that may build a namespaced name
//! (`scripts/lint.py abstract-sockets` enforces it): Windows named pipes, and
//! a probe for hosts that still listen on the old name so they fail with a
//! clear message.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
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

    /// Whether a host still listens on the name used before sockets moved into
    /// the private directory, i.e. an old build that this one cannot reach.
    /// Only for error messages: the connection is closed without a byte sent.
    pub fn legacy_listener_present(&self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            self.leaf
                .as_str()
                .to_ns_name::<GenericNamespaced>()
                .and_then(Stream::connect)
                .is_ok()
        }

        // macOS used `/tmp/<name>`; a probe there would connect to whatever
        // another user put in the shared directory, so it is not worth it.
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            false
        }
    }

    /// Suffix for a "socket not available" error when the likely reason is a
    /// host built before the move into the private directory.
    pub fn legacy_hint(&self) -> String {
        if self.legacy_listener_present() {
            format!(
                "; a host is listening on the old socket name '{}', update it to listen on {self}",
                self.leaf
            )
        } else {
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

/// [`verify_peer`] with the expected uid passed in, for tests. Windows has no
/// uid; pipe access is governed by the pipe's security descriptor instead.
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

/// The private directory that holds the sockets: `$DBFLUX_IPC_DIR`, else
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
            explicit: absolute_env(IPC_DIR_ENV),
            runtime: absolute_env("XDG_RUNTIME_DIR").or(session_default),
            cache: dirs::cache_dir(),
        }
    }
}

fn resolve_ipc_dir(sources: &DirSources, owner: u32) -> io::Result<PathBuf> {
    if let Some(explicit) = &sources.explicit {
        let (Some(parent), Some(leaf)) = (explicit.parent(), explicit.file_name()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{IPC_DIR_ENV}={} is not a usable directory",
                    explicit.display()
                ),
            ));
        };

        return private_dir(parent, &leaf.to_string_lossy(), owner);
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
    dbflux_core::local_files::private_dir_in_owned_by(parent, leaf, owner).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("IPC socket directory is not private: {error}"),
        )
    })
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
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::time::{Duration, Instant};

    /// A private temp dir removed on drop.
    struct Scratch(PathBuf);

    impl std::ops::Deref for Scratch {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for Scratch {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch_dir() -> Scratch {
        let dir = std::env::temp_dir().join(format!("dbx-ipc-t-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&dir).expect("scratch dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .expect("scratch dir mode");
        Scratch(dir)
    }

    fn sources(runtime: &Path, cache: &Path) -> DirSources {
        DirSources {
            explicit: None,
            runtime: Some(runtime.to_path_buf()),
            cache: Some(cache.to_path_buf()),
        }
    }

    /// Polls a nonblocking listener until `done` or a short deadline and
    /// returns every byte any connection delivered.
    fn drain_squatter(listener: &Listener, done: impl Fn() -> bool) -> Vec<u8> {
        let mut received = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);

        while Instant::now() < deadline {
            match listener.accept() {
                Ok(mut stream) => {
                    stream
                        .set_recv_timeout(Some(Duration::from_millis(500)))
                        .expect("recv timeout");
                    let mut buffer = [0_u8; 256];
                    while let Ok(read) = stream.read(&mut buffer) {
                        if read == 0 {
                            break;
                        }
                        received.extend_from_slice(buffer.get(..read).unwrap_or_default());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if done() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("squatter accept failed: {error}"),
            }
        }

        received
    }

    #[test]
    fn sockets_live_in_a_0700_dir_owned_by_this_user() {
        let runtime = scratch_dir();
        let cache = scratch_dir();

        let dir = resolve_ipc_dir(&sources(&runtime, &cache), own_uid()).expect("ipc dir");
        assert_eq!(dir, runtime.join(RUNTIME_SUBDIR));

        let metadata = std::fs::symlink_metadata(&dir).expect("dir metadata");
        assert!(metadata.file_type().is_dir());
        assert_eq!(metadata.uid(), own_uid());
        assert_eq!(metadata.mode() & 0o777, 0o700);

        let socket = IpcSocket::in_dir(&dir, "dbflux.sock".into()).expect("socket");
        assert_eq!(socket.path().parent(), Some(dir.as_path()));

        let _listener = socket
            .listen(ListenerNonblockingMode::Neither)
            .expect("listen");
        let file = std::fs::symlink_metadata(socket.path()).expect("socket file");
        assert_eq!(file.uid(), own_uid());
        assert_eq!(file.mode() & 0o077, 0, "socket file must be owner-only");
    }

    #[test]
    fn the_process_wide_dir_is_private_too() {
        let dir = isolate_ipc_dir_for_tests().expect("test dir");
        assert_eq!(ipc_dir().expect("ipc dir"), dir);

        let socket = app_control_socket().expect("socket");
        assert_eq!(socket.path().parent(), Some(dir.as_path()));
        assert_eq!(
            std::fs::metadata(&dir).expect("metadata").mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn falls_back_to_the_cache_dir_without_a_usable_runtime_dir() {
        let cache = scratch_dir();
        let unset = DirSources {
            explicit: None,
            runtime: None,
            cache: Some(cache.to_path_buf()),
        };
        assert_eq!(
            resolve_ipc_dir(&unset, own_uid()).expect("ipc dir"),
            cache.join(CACHE_SUBDIR)
        );

        // A runtime dir that is not a directory of ours is skipped, not used.
        let not_a_dir = cache.join("runtime-file");
        std::fs::write(&not_a_dir, b"").expect("plain file");
        assert_eq!(
            resolve_ipc_dir(&sources(&not_a_dir, &cache), own_uid()).expect("ipc dir"),
            cache.join(CACHE_SUBDIR)
        );
    }

    #[test]
    fn refuses_a_precreated_dir_that_others_can_write() {
        let runtime = scratch_dir();
        let cache = scratch_dir();
        let dir = runtime.join(RUNTIME_SUBDIR);
        std::fs::create_dir(&dir).expect("pre-created dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).expect("chmod 0777");

        let error = resolve_ipc_dir(&sources(&runtime, &cache), own_uid())
            .expect_err("a world-writable dir must be refused");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
        assert!(
            error.to_string().contains("can be written by other users"),
            "{error}"
        );
    }

    #[test]
    fn refuses_a_precreated_dir_owned_by_someone_else() {
        let runtime = scratch_dir();
        std::fs::create_dir(runtime.join(RUNTIME_SUBDIR)).expect("pre-created dir");

        // Tests cannot chown, so the expected owner is the one that differs.
        let error = private_dir(&runtime, RUNTIME_SUBDIR, own_uid().wrapping_add(1))
            .expect_err("a dir of another user must be refused");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
        assert!(
            error.to_string().contains("belongs to another user"),
            "{error}"
        );
    }

    #[test]
    fn refuses_a_symlink_in_place_of_the_dir() {
        let runtime = scratch_dir();
        let cache = scratch_dir();
        let target = scratch_dir();
        std::os::unix::fs::symlink(&target, runtime.join(RUNTIME_SUBDIR)).expect("symlink");

        let error = resolve_ipc_dir(&sources(&runtime, &cache), own_uid())
            .expect_err("a symlink must be refused");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
        assert!(error.to_string().contains("is not a directory"), "{error}");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn a_squatter_on_the_legacy_abstract_name_receives_nothing() {
        let dir = scratch_dir();
        let socket = IpcSocket::in_dir(
            &dir,
            format!("dbx-squat-{}.sock", uuid::Uuid::new_v4().simple()),
        )
        .expect("socket");
        let squatter = bind_legacy_name_for_tests(&socket).expect("squatter binds");

        let client = std::thread::spawn({
            let socket = socket.clone();
            move || match socket.connect() {
                Ok(mut stream) => {
                    let _ = stream.write_all(b"TOKEN-and-SECRET");
                    true
                }
                Err(_) => false,
            }
        });

        let received = drain_squatter(&squatter, || client.is_finished());
        let connected = client.join().expect("client thread");

        assert!(received.is_empty(), "squatter received {received:?}");
        assert!(!connected, "nothing listens at {socket}, connect must fail");
        assert!(socket.legacy_listener_present());
    }

    #[test]
    fn a_listener_of_another_uid_at_the_path_receives_nothing() {
        let dir = scratch_dir();
        let socket = IpcSocket::in_dir(&dir, "dbx-squat.sock".into()).expect("socket");
        let squatter = socket
            .listen(ListenerNonblockingMode::Accept)
            .expect("squatter binds");

        let client = std::thread::spawn({
            let socket = socket.clone();
            move || match socket.connect_expecting_peer(own_uid().wrapping_add(1)) {
                Ok(mut stream) => {
                    let _ = stream.write_all(b"TOKEN-and-SECRET");
                    Ok(())
                }
                Err(error) => Err(error),
            }
        });

        let received = drain_squatter(&squatter, || client.is_finished());
        let result = client.join().expect("client thread");

        assert!(received.is_empty(), "squatter received {received:?}");
        let error = result.expect_err("a server of another uid must be refused");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
    }

    #[test]
    fn servers_refuse_a_client_of_another_uid() {
        let dir = scratch_dir();
        let socket = IpcSocket::in_dir(&dir, "dbx-peer.sock".into()).expect("socket");
        let listener = socket
            .listen(ListenerNonblockingMode::Neither)
            .expect("listen");

        let client = std::thread::spawn({
            let socket = socket.clone();
            move || socket.connect().expect("same-uid server is accepted")
        });
        let accepted = listener.accept().expect("accept");
        let _client = client.join().expect("client thread");

        verify_peer(&accepted).expect("same uid passes");
        let error =
            verify_peer_as(&accepted, own_uid().wrapping_add(1)).expect_err("other uid refused");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        assert!(check_peer_uid(None, own_uid()).is_err());
        assert!(check_peer_uid(Some(own_uid()), own_uid()).is_ok());
    }

    #[test]
    fn a_stale_socket_file_of_ours_is_replaced_but_a_live_one_is_not() {
        let dir = scratch_dir();
        let socket = IpcSocket::in_dir(&dir, "dbx-stale.sock".into()).expect("socket");

        let first = socket
            .listen(ListenerNonblockingMode::Neither)
            .expect("first bind");
        let error = socket
            .listen(ListenerNonblockingMode::Neither)
            .expect_err("a live socket must not be displaced");
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse, "{error}");
        drop(first);

        // A crashed process leaves its socket file behind.
        let stale = std::os::unix::net::UnixListener::bind(socket.path()).expect("stale bind");
        drop(stale);
        assert!(socket.path().exists(), "std leaves the socket file");
        let _second = socket
            .listen(ListenerNonblockingMode::Neither)
            .expect("a stale socket of ours is replaced");
    }

    #[test]
    fn a_non_socket_file_at_the_path_is_never_removed() {
        let dir = scratch_dir();
        let socket = IpcSocket::in_dir(&dir, "dbx-file.sock".into()).expect("socket");
        std::fs::write(socket.path(), b"not a socket").expect("plain file");

        let error = socket
            .listen(ListenerNonblockingMode::Neither)
            .expect_err("a plain file must not be replaced");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{error}");
        assert_eq!(
            std::fs::read(socket.path()).expect("file kept"),
            b"not a socket"
        );
    }

    #[test]
    fn socket_names_cannot_leave_the_directory() {
        let dir = scratch_dir();
        for bad in ["", ".", "..", "a/b", "../x", "a\\b", "a\0b"] {
            assert!(IpcSocket::in_dir(&dir, bad.into()).is_err(), "{bad:?}");
        }

        let long = "x".repeat(MAX_SOCKET_PATH_BYTES);
        let error = IpcSocket::in_dir(&dir, long).expect_err("too long");
        assert!(error.to_string().contains("limit"), "{error}");
    }
}
