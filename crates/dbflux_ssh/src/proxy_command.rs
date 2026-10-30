//! `ProxyCommand` transport: the SSH session runs over the command's stdin
//! and stdout instead of a TCP socket, as OpenSSH does.
//!
//! The command gets one end of a Unix socket pair as both stdin and stdout;
//! libssh2 gets the other end. The command is run as `$SHELL -c "exec ..."`
//! (`/bin/sh` when `SHELL` is unset), matching OpenSSH's
//! `ssh_proxy_connect`. Its stderr is forwarded to the log line by line,
//! because access proxies print login prompts and URLs there.
//!
//! The expanded command can carry credentials (an access token on the
//! command line), so only its program name is logged at info level and put
//! in errors; the full command is logged at debug level only.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use dbflux_core::DbError;

/// The libssh2 side of a running `ProxyCommand`.
///
/// Owned by the `Session`, which drops it after freeing the libssh2 session;
/// dropping it kills and reaps the command.
pub(crate) struct ProxyStream {
    socket: UnixStream,
    child: Child,
}

impl AsRawFd for ProxyStream {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

impl Drop for ProxyStream {
    fn drop(&mut self) {
        // The command may already have exited after its stdin closed.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts an already token-expanded `ProxyCommand`.
pub(crate) fn spawn(command: &str, io_timeout: Duration) -> Result<ProxyStream, DbError> {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    spawn_with_shell(&shell, command, io_timeout)
}

fn spawn_with_shell(
    shell: &OsStr,
    command: &str,
    io_timeout: Duration,
) -> Result<ProxyStream, DbError> {
    let (socket, command_end) = UnixStream::pair().map_err(|e| {
        DbError::connection_failed(format!("Failed to create the ProxyCommand socket: {e}"))
    })?;
    let command_stdin = command_end.try_clone().map_err(|e| {
        DbError::connection_failed(format!("Failed to create the ProxyCommand socket: {e}"))
    })?;

    let program = program_name(command);
    log::info!("[SSH] Starting ProxyCommand '{program}'");
    log::debug!("[SSH] ProxyCommand command line: {command}");

    let mut child = Command::new(shell)
        .arg("-c")
        .arg(format!("exec {command}"))
        .stdin(Stdio::from(OwnedFd::from(command_stdin)))
        .stdout(Stdio::from(OwnedFd::from(command_end)))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            DbError::connection_failed(format!(
                "Failed to start ProxyCommand '{program}' with {}: {e}",
                Path::new(shell).display()
            ))
        })?;

    if let Some(stderr) = child.stderr.take() {
        std::thread::Builder::new()
            .name("ssh-proxy-command-stderr".to_string())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::info!("[SSH] ProxyCommand stderr: {line}");
                }
            })
            .ok();
    }

    socket.set_read_timeout(Some(io_timeout)).ok();
    socket.set_write_timeout(Some(io_timeout)).ok();

    Ok(ProxyStream { socket, child })
}

/// The program a `ProxyCommand` runs, without its directory, arguments or
/// leading `NAME=value` assignments: the part that is safe to log and show.
pub(crate) fn program_name(command: &str) -> &str {
    command
        .split_whitespace()
        .find(|word| !word.contains('='))
        .map(|word| word.trim_matches(|c| c == '\'' || c == '"'))
        .map(|word| word.rsplit('/').next().unwrap_or(word))
        .filter(|name| !name.is_empty())
        .unwrap_or("?")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn command_speaks_over_its_stdin_and_stdout() {
        let mut stream = spawn("cat", Duration::from_secs(5)).unwrap();

        stream.socket.write_all(b"SSH-2.0-probe\r\n").unwrap();
        let mut echoed = [0u8; 15];
        stream.socket.read_exact(&mut echoed).unwrap();

        assert_eq!(&echoed, b"SSH-2.0-probe\r\n");
    }

    #[test]
    fn dropping_the_stream_kills_the_command() {
        let stream = spawn("sleep 30", Duration::from_secs(5)).unwrap();
        let pid = stream.child.id();

        drop(stream);

        // Killed and reaped by Drop, so signal 0 finds no such process.
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "pid {pid} still alive");
    }

    #[test]
    fn a_command_that_exits_reads_as_end_of_stream() {
        let mut stream = spawn("true", Duration::from_secs(5)).unwrap();

        let mut buf = [0u8; 1];
        let read = stream.socket.read(&mut buf).unwrap();

        assert_eq!(read, 0);
    }

    #[test]
    fn program_name_drops_the_arguments_path_and_assignments() {
        assert_eq!(
            program_name(
                "TUNNEL_SERVICE_TOKEN_SECRET=s3cr3t /usr/local/bin/cloudflared access ssh --hostname h --service-token-secret s3cr3t"
            ),
            "cloudflared"
        );
        assert_eq!(program_name("nc -w 1 host 22"), "nc");
        assert_eq!(program_name("'/opt/tools/run' --flag"), "run");
        assert_eq!(program_name("   "), "?");
    }

    #[test]
    fn a_start_failure_names_the_program_but_not_the_command() {
        let error = match spawn_with_shell(
            OsStr::new("/nonexistent/dbflux-test-shell"),
            "cloudflared access ssh --service-token-secret s3cr3t",
            Duration::from_secs(5),
        ) {
            Ok(_) => panic!("a missing shell cannot start the command"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("'cloudflared'"), "got: {error}");
        assert!(!error.contains("s3cr3t"), "got: {error}");
    }
}
