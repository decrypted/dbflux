//! `ProxyCommand` transport: the SSH session runs over the command's stdin
//! and stdout instead of a TCP socket, as OpenSSH does.
//!
//! The command gets one end of a Unix socket pair as both stdin and stdout;
//! libssh2 gets the other end. The command is run as `$SHELL -c "exec ..."`
//! (`/bin/sh` when `SHELL` is unset), matching OpenSSH's
//! `ssh_proxy_connect`.
//!
//! The expanded command can carry credentials (an access token on the
//! command line), so only its program name is logged at info level and put
//! in errors; the full command is logged at debug level only. The same goes
//! for its stderr: access proxies print one-time login URLs there and
//! wrapper scripts echo tokens, while info lines are stored in the audit
//! database. Stderr is therefore logged at debug, and only its last few
//! lines, redacted, go into the error of a failed connect.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use dbflux_core::DbError;

/// Stderr lines kept for the error message, and the length each is cut to.
const STDERR_TAIL_LINES: usize = 3;
const STDERR_TAIL_LINE_CHARS: usize = 200;

/// How long a failed connect waits for the command's last stderr lines; a
/// command that exits has usually written them already.
const STDERR_TAIL_WAIT: Duration = Duration::from_millis(250);

/// The last few stderr lines of a `ProxyCommand`, shared between the reader
/// thread and whoever reports a failed connect.
#[derive(Clone, Default)]
pub(crate) struct StderrTail(Arc<(Mutex<TailState>, Condvar)>);

#[derive(Default)]
struct TailState {
    lines: std::collections::VecDeque<String>,
    closed: bool,
}

impl StderrTail {
    fn push(&self, line: &str) {
        let (state, _) = &*self.0;
        let Ok(mut state) = state.lock() else {
            return;
        };
        if state.lines.len() == STDERR_TAIL_LINES {
            state.lines.pop_front();
        }
        state.lines.push_back(tail_line(line));
    }

    fn close(&self) {
        let (state, closed) = &*self.0;
        if let Ok(mut state) = state.lock() {
            state.closed = true;
        }
        closed.notify_all();
    }

    /// `ProxyCommand stderr: a | b | c`, or `None` when the command printed
    /// nothing. Waits up to `wait` for stderr to close first.
    pub(crate) fn summary(&self, wait: Duration) -> Option<String> {
        let (state, closed) = &*self.0;
        let state = state.lock().ok()?;
        let state = closed
            .wait_timeout_while(state, wait, |state| !state.closed)
            .map(|(state, _)| state)
            .ok()?;

        let lines: Vec<&str> = state
            .lines
            .iter()
            .map(String::as_str)
            .filter(|line| !line.is_empty())
            .collect();
        if lines.is_empty() {
            return None;
        }

        Some(format!("ProxyCommand stderr: {}", lines.join(" | ")))
    }

    /// The summary for the error of a failed connect.
    pub(crate) fn for_error(&self) -> Option<String> {
        self.summary(STDERR_TAIL_WAIT)
    }
}

/// A stderr line as it may appear in an error.
///
/// The error of a failed connect is stored in the audit log, and a login or
/// device URL can carry its one-time code anywhere after its host. So the
/// line is redacted first (`key=value` credentials, URL userinfo), and only
/// then is every URL cut to its host, with or without a scheme and also when
/// JSON-escaped (`https:\/\/`), and any other query string dropped. Dropping
/// text after redaction can only remove more, never undo a redaction. The
/// line is shortened last, so a cut cannot hide a secret from either step.
fn tail_line(line: &str) -> String {
    let unescaped = line.trim_end().replace("\\/", "/");
    let redacted = dbflux_core::sanitize_uri(&unescaped);
    shorten(&reduce_bare_urls(&reduce_urls_to_host(&redacted)))
}

fn is_word_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>')
}

/// Per word: a URL without a scheme keeps only its host, and any other word
/// drops what follows a `?` that starts a query (`x?code=...`).
///
/// A word counts as such a URL when, after leading brackets or separators
/// (`(`, `[`, `{`, `=`, `,`, `:`) or a `key=` prefix and any `user[:pass]@`,
/// its host is a dotted name, an IPv4 address or `localhost`, optionally with
/// a port, and something follows it (`/`, `?` or `#`). An IPv4 network such
/// as `10.0.0.0/8` is kept.
fn reduce_bare_urls(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;

    while !rest.is_empty() {
        let word_start = rest
            .find(|c: char| !is_word_separator(c))
            .unwrap_or(rest.len());
        let (separators, after) = rest.split_at(word_start);
        output.push_str(separators);

        let word_end = after.find(is_word_separator).unwrap_or(after.len());
        let (word, remainder) = after.split_at(word_end);
        output.push_str(&reduce_bare_url_word(word));
        rest = remainder;
    }

    output
}

fn reduce_bare_url_word(word: &str) -> String {
    let lead = word.len()
        - word
            .trim_start_matches(['(', '[', '{', '=', ',', ':'])
            .len();
    let (prefix, core) = word.split_at(lead);
    if let Some(reduced) = reduce_bare_url(core) {
        return format!("{prefix}{reduced}");
    }
    // `url=login.example.com/...`
    if let Some((key, value)) = core.split_once('=')
        && let Some(reduced) = reduce_bare_url(value)
    {
        return format!("{prefix}{key}={reduced}");
    }

    match word.split_once('?') {
        Some((before, query)) if query.contains('=') => format!("{before}?…"),
        _ => word.to_string(),
    }
}

/// `host/…` for a scheme-less URL, without its user info, or `None`.
fn reduce_bare_url(candidate: &str) -> Option<String> {
    let host_end = candidate.find(['/', '?', '#']).unwrap_or(candidate.len());
    let (authority, location) = candidate.split_at(host_end);
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let has_user_info = host.len() != authority.len();

    if !(is_host_name(host) || is_localhost(host)) {
        return None;
    }
    if location.is_empty() {
        // Only the user info needs to go.
        return has_user_info.then(|| host.to_string());
    }
    if is_ipv4_network(host, location) {
        return None;
    }
    Some(format!("{host}/…"))
}

fn is_localhost(candidate: &str) -> bool {
    let name = candidate
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(candidate, |(name, _)| name);
    name.eq_ignore_ascii_case("localhost")
}

/// `10.0.0.0/8`: an address with a prefix length, not a URL.
fn is_ipv4_network(host: &str, location: &str) -> bool {
    host.split('.').count() == 4
        && host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && location.strip_prefix('/').is_some_and(|mask| {
            (1..=2).contains(&mask.len()) && mask.bytes().all(|b| b.is_ascii_digit())
        })
}

/// `example.com`, `login.example.com:8443` or `10.0.0.1`: dot-separated
/// labels ending in an alphabetic top-level label, or four numeric labels.
fn is_host_name(candidate: &str) -> bool {
    let name = candidate
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(candidate, |(name, _)| name);
    let labels: Vec<&str> = name.split('.').collect();
    if labels.len() < 2
        || labels.iter().any(|label| {
            label.is_empty()
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return false;
    }

    let numeric = labels
        .iter()
        .all(|label| label.bytes().all(|b| b.is_ascii_digit()));
    let top = labels.last().copied().unwrap_or_default();
    (numeric && labels.len() == 4)
        || (top.len() >= 2 && top.bytes().all(|b| b.is_ascii_alphabetic()))
}

/// Replaces every `scheme://[user@]host...` with `scheme://host/…`.
fn reduce_urls_to_host(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(separator) = rest.find("://") {
        let (before, after) = rest.split_at(separator + 3);
        output.push_str(before);

        let url_end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>'))
            .unwrap_or(after.len());
        let (url, remainder) = after.split_at(url_end);

        let authority_end = url.find(['/', '?', '#']).unwrap_or(url.len());
        let (authority, location) = url.split_at(authority_end);
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        output.push_str(host);
        if !location.is_empty() {
            output.push_str("/…");
        }

        rest = remainder;
    }

    output.push_str(rest);
    output
}

fn shorten(line: &str) -> String {
    match line.char_indices().nth(STDERR_TAIL_LINE_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// The libssh2 side of a running `ProxyCommand`.
///
/// Owned by the `Session`, which drops it after freeing the libssh2 session;
/// dropping it kills and reaps the command.
pub(crate) struct ProxyStream {
    socket: UnixStream,
    child: Child,
    stderr_tail: StderrTail,
}

impl ProxyStream {
    pub(crate) fn stderr_tail(&self) -> StderrTail {
        self.stderr_tail.clone()
    }
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

    let stderr_tail = StderrTail::default();
    match child.stderr.take() {
        Some(stderr) => {
            let tail = stderr_tail.clone();
            let reader = std::thread::Builder::new()
                .name("ssh-proxy-command-stderr".to_string())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        log::debug!("[SSH] ProxyCommand stderr: {line}");
                        tail.push(&line);
                    }
                    tail.close();
                });
            if reader.is_err() {
                stderr_tail.close();
            }
        }
        None => stderr_tail.close(),
    }

    socket.set_read_timeout(Some(io_timeout)).ok();
    socket.set_write_timeout(Some(io_timeout)).ok();

    Ok(ProxyStream {
        socket,
        child,
        stderr_tail,
    })
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

    struct CapturingLogger(Mutex<Vec<(log::Level, String)>>);

    impl log::Log for CapturingLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            if let Ok(mut records) = self.0.lock() {
                records.push((record.level(), record.args().to_string()));
            }
        }

        fn flush(&self) {}
    }

    static LOGGER: CapturingLogger = CapturingLogger(Mutex::new(Vec::new()));

    /// Info lines are stored in the audit database, and proxies print login
    /// URLs and tokens on stderr.
    #[test]
    fn stderr_is_logged_at_debug_only() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            log::set_logger(&LOGGER).expect("no other logger is installed in this test binary");
        });
        log::set_max_level(log::LevelFilter::Trace);

        let marker = "dbflux-proxy-stderr-marker";
        let stream = spawn(
            &format!("sh -c 'echo {marker} token=s3cr3t >&2'"),
            Duration::from_secs(5),
        )
        .unwrap();
        stream.stderr_tail().summary(Duration::from_secs(5));
        drop(stream);

        let records: Vec<_> = LOGGER
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, message)| message.contains(marker))
            .cloned()
            .collect();
        assert!(
            records
                .iter()
                .any(|(_, message)| message.starts_with("[SSH] ProxyCommand stderr:")),
            "the stderr line was never logged: {records:?}"
        );
        for (level, message) in &records {
            assert_eq!(*level, log::Level::Debug, "{message}");
        }
    }

    #[test]
    fn the_error_tail_keeps_the_last_lines_short_and_redacted() {
        let long = "x".repeat(500);
        let stream = spawn(
            &format!(
                "sh -c 'for n in 1 2 3 4 5; do echo line$n >&2; done; \
                 echo {long} >&2; echo \"login https://h.example/cb?token=s3cr3t\" >&2'"
            ),
            Duration::from_secs(5),
        )
        .unwrap();

        let summary = stream
            .stderr_tail()
            .summary(Duration::from_secs(5))
            .expect("the command printed to stderr");

        assert!(
            summary.starts_with("ProxyCommand stderr: line5 | "),
            "{summary}"
        );
        assert!(!summary.contains("line4"), "{summary}");
        assert!(
            summary.contains(&format!("{}…", "x".repeat(200))),
            "{summary}"
        );
        assert!(!summary.contains(&"x".repeat(201)), "{summary}");
        assert!(summary.contains("login https://h.example/…"), "{summary}");
        assert!(!summary.contains("s3cr3t"), "{summary}");
    }

    /// One-time codes ride in URL paths and in parameters no list knows.
    #[test]
    fn tail_lines_keep_only_the_host_of_urls() {
        assert_eq!(
            tail_line("open https://login.example/device/ABCD-1234 to continue"),
            "open https://login.example/… to continue"
        );
        assert_eq!(
            tail_line("callback \"http://127.0.0.1:8080/cb?user_code=XYZ#f\" ready"),
            "callback \"http://127.0.0.1:8080/…\" ready"
        );
        assert_eq!(tail_line("via ssh://alice:pw@bastion"), "via ssh://bastion");
        assert_eq!(
            tail_line("auth failed: password=hunter2"),
            "auth failed: password=***"
        );
        assert_eq!(tail_line("no url here"), "no url here");
        assert_eq!(
            tail_line(
                r#"{"verification_uri_complete":"https:\/\/device.sso.example\/?user_code=WDJB-MJHT"}"#
            ),
            r#"{"verification_uri_complete":"https://device.sso.example/…"}"#
        );
        assert_eq!(
            tail_line("visit login.example.com/activate?code=WDJB-MJHT now"),
            "visit login.example.com/… now"
        );
        assert_eq!(tail_line("are you sure? yes"), "are you sure? yes");
    }

    /// Redaction runs before anything is dropped: dropping a query cannot
    /// remove a key and leave its quoted value behind.
    #[test]
    fn tail_lines_are_redacted_before_query_strings_are_dropped() {
        let quoted = tail_line(r#"auth failed for x?password="hunter2""#);
        assert!(!quoted.contains("hunter2"), "{quoted}");

        let ado = tail_line(r#"Server=db?app=x;Password="hunter 2""#);
        assert!(!ado.contains("hunter 2"), "{ado}");
    }

    /// A URL without a scheme loses its path and fragment too.
    #[test]
    fn bare_urls_keep_only_their_host() {
        assert_eq!(
            tail_line("visit login.example.com/activate/WDJB-MJHT now"),
            "visit login.example.com/… now"
        );
        assert_eq!(
            tail_line("open login.example.com#WDJB-MJHT"),
            "open login.example.com/…"
        );
        assert_eq!(
            tail_line("proxy 10.0.0.1:8080/cb?x=1"),
            "proxy 10.0.0.1:8080/…"
        );
        assert_eq!(
            tail_line("read /home/alice/x.sql and v1.2/notes, error #42"),
            "read /home/alice/x.sql and v1.2/notes, error #42"
        );
    }

    /// Brackets, `key=` prefixes, user info and `localhost` do not hide a URL.
    #[test]
    fn bare_urls_in_common_wrappings_keep_only_their_host() {
        assert_eq!(
            tail_line("(login.example.com/activate/WDJB-MJHT)"),
            "(login.example.com/…"
        );
        assert_eq!(
            tail_line("[login.example.com/x/WDJB]"),
            "[login.example.com/…"
        );
        assert_eq!(
            tail_line("url=login.example.com/device/WDJB-MJHT"),
            "url=login.example.com/…"
        );
        assert_eq!(
            tail_line("open alice@login.example.com/activate/WDJB-MJHT"),
            "open login.example.com/…"
        );
        assert_eq!(
            tail_line("dial alice:hunter2@db.example.com:5432/app"),
            "dial db.example.com:5432/…"
        );
        assert_eq!(
            tail_line("as alice:hunter2@db.example.com"),
            "as db.example.com"
        );
        assert_eq!(
            tail_line("localhost:8080/device/WDJB-MJHT"),
            "localhost:8080/…"
        );
        assert_eq!(
            tail_line("localhost:8080/activate?WDJB-MJHT"),
            "localhost:8080/…"
        );
        assert_eq!(
            tail_line("localhost:8080/cb#access_token=XYZ"),
            "localhost:8080/…"
        );
        assert_eq!(tail_line("allow 10.0.0.0/8 only"), "allow 10.0.0.0/8 only");
    }

    /// Redaction runs before the cut, so a secret past the cut point cannot
    /// lose the part that marks it as one.
    #[test]
    fn tail_lines_are_redacted_before_they_are_shortened() {
        let line = format!(
            "{} https://alice:hunter2@h.example/x",
            "x".repeat(STDERR_TAIL_LINE_CHARS - 20)
        );

        let tail = tail_line(&line);

        assert!(!tail.contains("hunter2"), "{tail}");
        assert!(!tail.contains("alice"), "{tail}");
    }

    #[test]
    fn a_silent_command_has_no_error_tail() {
        let stream = spawn("true", Duration::from_secs(5)).unwrap();

        assert_eq!(stream.stderr_tail().summary(Duration::from_secs(5)), None);
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
