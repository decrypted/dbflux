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
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("scratch dir {} not removed: {error}", self.0.display());
        }
    }
}

fn scratch_dir() -> Scratch {
    // Canonical so that comparisons hold where the temp dir sits below a link
    // (`/var` -> `/private/var` on macOS).
    let base = std::env::temp_dir().canonicalize().expect("temp dir");
    let dir = base.join(format!("dbx-ipc-t-{}", uuid::Uuid::new_v4().simple()));
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
                stream
                    .write_all(b"TOKEN-and-SECRET")
                    .expect("the write reaches the peer");
                true
            }
            Err(_) => false,
        }
    });

    let received = drain_squatter(&squatter, || client.is_finished());
    let connected = client.join().expect("client thread");

    assert!(received.is_empty(), "squatter received {received:?}");
    assert!(!connected, "nothing listens at {socket}, connect must fail");
}

/// The hint for an old host must not connect to the old name: anyone can bind
/// it, and a listener with a full backlog would block that connect forever.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn the_legacy_hint_never_connects_to_the_old_name() {
    let dir = scratch_dir();
    let socket = IpcSocket::in_dir(
        &dir,
        format!("dbx-hint-{}.sock", uuid::Uuid::new_v4().simple()),
    )
    .expect("socket");
    let squatter = bind_legacy_name_for_tests(&socket).expect("squatter binds");

    let hint = socket.legacy_hint();

    assert!(hint.contains("old socket name"), "{hint}");
    match squatter.accept() {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("building the hint connected to the old name"),
        Err(error) => panic!("squatter accept failed: {error}"),
    }
}

#[test]
fn an_explicit_dir_is_used_at_its_exact_path() {
    let base = scratch_dir();
    let explicit = base.join(".dbflux-ipc:test");
    let sources = DirSources {
        explicit: Some(explicit.clone()),
        runtime: None,
        cache: None,
    };

    let dir = resolve_ipc_dir(&sources, own_uid()).expect("explicit dir");
    assert_eq!(dir, explicit, "the configured path must not be rewritten");
    assert_eq!(
        std::fs::metadata(&dir).expect("metadata").mode() & 0o777,
        0o700
    );

    let relative = DirSources {
        explicit: Some(PathBuf::from("relative/ipc")),
        runtime: None,
        cache: None,
    };
    let error = resolve_ipc_dir(&relative, own_uid()).expect_err("a relative dir is refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{error}");

    let foreign = DirSources {
        explicit: Some(base.join("foreign")),
        runtime: None,
        cache: None,
    };
    std::fs::create_dir(base.join("foreign")).expect("pre-created dir");
    let error = resolve_ipc_dir(&foreign, own_uid().wrapping_add(1))
        .expect_err("an explicit dir of another user is refused");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
}
fn explicit(path: PathBuf) -> DirSources {
    DirSources {
        explicit: Some(path),
        runtime: None,
        cache: None,
    }
}

/// With a trailing `/` the final lstat follows a symlink, so the link's 0700
/// target would pass the checks meant for the link itself.
#[test]
fn an_explicit_dir_with_a_trailing_slash_cannot_hide_a_symlink() {
    let base = scratch_dir();
    let target = base.join("target");
    std::fs::create_dir(&target).expect("target dir");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    std::os::unix::fs::symlink(&target, base.join("link")).expect("symlink");

    let mut with_slash = base.join("link").into_os_string();
    with_slash.push("/");
    let error = resolve_ipc_dir(&explicit(PathBuf::from(with_slash)), own_uid())
        .expect_err("a trailing slash must be refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{error}");

    let error = resolve_ipc_dir(&explicit(base.join("link")), own_uid())
        .expect_err("the link itself is refused");
    assert!(error.to_string().contains("is not a directory"), "{error}");
}

#[test]
fn an_explicit_dir_must_name_a_plain_entry() {
    let base = scratch_dir();
    std::fs::create_dir(base.join("a")).expect("dir");
    std::fs::set_permissions(base.join("a"), std::fs::Permissions::from_mode(0o755))
        .expect("chmod");

    for bad in [
        base.join("a").join(".."),
        base.join("a").join("."),
        base.join("a").join("..").join("b"),
        PathBuf::from("/"),
    ] {
        let error = resolve_ipc_dir(&explicit(bad.clone()), own_uid())
            .expect_err("a dot component or the root must be refused");
        assert_eq!(
            error.kind(),
            io::ErrorKind::InvalidInput,
            "{bad:?}: {error}"
        );
    }

    // `a/.` names `a` itself: it must stay as it was, not be narrowed to 0700.
    assert_eq!(
        std::fs::metadata(base.join("a")).expect("a").mode() & 0o777,
        0o755
    );
}

#[test]
fn an_explicit_dir_below_a_dir_others_can_write_is_refused() {
    let base = scratch_dir();
    let open = base.join("open");
    std::fs::create_dir(&open).expect("open dir");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).expect("chmod 0777");

    let error = resolve_ipc_dir(&explicit(open.join("ipc")), own_uid())
        .expect_err("another user could swap the entry in a writable ancestor");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
    assert!(!open.join("ipc").exists(), "nothing is created there");

    // Sticky, like /tmp: others cannot rename an entry they do not own.
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o1777)).expect("chmod 1777");
    assert_eq!(
        resolve_ipc_dir(&explicit(open.join("ipc")), own_uid()).expect("sticky ancestor"),
        open.join("ipc")
    );
}

#[test]
fn a_symlink_above_the_explicit_dir_is_checked_but_the_path_kept() {
    let base = scratch_dir();
    std::fs::create_dir(base.join("real")).expect("real dir");
    // Independent of the umask and of whether this user has a private group.
    std::fs::set_permissions(base.join("real"), std::fs::Permissions::from_mode(0o755))
        .expect("chmod 0755");
    std::os::unix::fs::symlink(base.join("real"), base.join("alias")).expect("symlink");

    let configured = base.join("alias").join("ipc");
    let dir = resolve_ipc_dir(&explicit(configured.clone()), own_uid())
        .expect("a linked ancestor of ours is fine");
    assert_eq!(dir, configured, "the configured path must not be rewritten");
    assert!(base.join("real").join("ipc").is_dir());

    // The link's target is checked as well as the link.
    std::fs::set_permissions(base.join("real"), std::fs::Permissions::from_mode(0o777))
        .expect("chmod 0777");
    let error = resolve_ipc_dir(&explicit(base.join("alias").join("ipc2")), own_uid())
        .expect_err("a world-writable link target is refused");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
}

#[test]
fn a_missing_parent_of_the_explicit_dir_names_the_variable() {
    let base = scratch_dir();
    let error = resolve_ipc_dir(&explicit(base.join("missing").join("ipc")), own_uid())
        .expect_err("the parent must exist");
    assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
    assert!(error.to_string().contains(IPC_DIR_ENV), "{error}");
    assert!(error.to_string().contains("missing"), "{error}");
}

#[test]
fn group_write_above_the_dir_is_trusted_only_for_a_private_group() {
    let me = 1000;
    let other = 1001;

    // umask 002 with a per-user group: the user's own folders.
    assert!(ancestor_is_trusted(me, me, 0o40775, false, me));
    // A shared group of the user, a root-owned group-writable dir
    // (`root:staff /usr/local`), another user's dir.
    assert!(!ancestor_is_trusted(me, 50, 0o42775, false, me));
    assert!(!ancestor_is_trusted(0, 50, 0o40775, false, me));
    assert!(!ancestor_is_trusted(other, other, 0o40755, false, me));
    // World write is refused unless sticky, like /tmp.
    assert!(!ancestor_is_trusted(me, me, 0o40777, false, me));
    assert!(ancestor_is_trusted(0, 0, 0o41777, false, me));
    // Plain dirs of the user or root.
    assert!(ancestor_is_trusted(me, 50, 0o40755, false, me));
    assert!(ancestor_is_trusted(0, 0, 0o40755, false, me));
    // A link: only its owner matters, its mode is always 0777.
    assert!(ancestor_is_trusted(0, 0, 0o120777, true, me));
    assert!(!ancestor_is_trusted(other, other, 0o120777, true, me));
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
                stream
                    .write_all(b"TOKEN-and-SECRET")
                    .expect("the write reaches the peer");
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
