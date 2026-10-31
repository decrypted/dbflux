//! SSH host key verification for DBFlux tunnels.
//!
//! A host key is trusted when the user's OpenSSH known_hosts files vouch for
//! it, or when DBFlux pinned it earlier in its own store. A key nobody knows
//! yet is never accepted silently: the connect fails with an error the GUI
//! turns into a "Trust and remember" prompt, unless the user opted into
//! accepting new keys (`StrictHostKeyChecking accept-new`/`no`, or the global
//! setting) and known_hosts holds no key of another type for the host. A
//! changed or revoked key always fails.
//!
//! The user's known_hosts files are only ever read; accepted keys go to
//! DBFlux's own store.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use dbflux_core::DbError;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use ssh2::{MethodType, Session};

use crate::ssh_config::{HostKeyOptions, StrictHostKeyChecking, glob_matches};
use crate::{
    load_tofu_known_hosts, save_tofu_known_hosts, sha256_base64_fingerprint, tofu_known_hosts_path,
};

/// known_hosts writers pad their base64; hand-edited files sometimes do not.
const KNOWN_HOSTS_BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

const DEFAULT_SSH_PORT: u16 = 22;

// ---------------------------------------------------------------------------
// known_hosts parsing
// ---------------------------------------------------------------------------

/// The marker at the start of a known_hosts line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownHostMarker {
    None,
    /// `@cert-authority`: the key signs host certificates. DBFlux does not
    /// verify host certificates, so such a line never vouches for a host key.
    CertAuthority,
    /// `@revoked`: the key must never be accepted.
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostField {
    /// Comma-separated patterns, `*`/`?` globs and `!` negation.
    Patterns(Vec<String>),
    /// `|1|salt|hash`: HMAC-SHA1 of the lookup name keyed by the salt.
    Hashed { salt: Vec<u8>, hash: Vec<u8> },
}

/// One usable line of a known_hosts file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownHostEntry {
    pub marker: KnownHostMarker,
    hosts: HostField,
    /// The key type the key blob itself names, e.g. `ssh-ed25519`.
    pub key_type: String,
    pub key: Vec<u8>,
    /// 1-based line number, for error messages.
    pub line: usize,
}

impl KnownHostEntry {
    /// Whether this line applies to `name` (a [`known_hosts_lookup_name`]).
    pub fn matches_host(&self, name: &str) -> bool {
        match &self.hosts {
            HostField::Patterns(patterns) => {
                let name = name.to_lowercase();
                let mut positive = false;
                for pattern in patterns {
                    let pattern = pattern.to_lowercase();
                    match pattern.strip_prefix('!') {
                        Some(negated) if glob_matches(negated, &name) => return false,
                        Some(_) => {}
                        None => positive |= glob_matches(&pattern, &name),
                    }
                }
                positive
            }
            // ssh hashes the name it looked up, which it lowercases for host
            // names but not for a HostKeyAlias; try both spellings.
            HostField::Hashed { salt, hash } => {
                hashed_name_matches(salt, hash, name)
                    || hashed_name_matches(salt, hash, &name.to_lowercase())
            }
        }
    }
}

fn hashed_name_matches(salt: &[u8], hash: &[u8], name: &str) -> bool {
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(salt) else {
        return false;
    };
    mac.update(name.as_bytes());
    mac.verify_slice(hash).is_ok()
}

/// Parses known_hosts text the way OpenSSH's `hostfile.c` reads it. Lines
/// that are comments, blank, or malformed (an unknown `@` marker, bad
/// base64, a key whose blob names another type than the line, the legacy
/// SSH-1 `bits exponent modulus` form) are skipped, never guessed at.
pub fn parse_known_hosts(text: &str) -> Vec<KnownHostEntry> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| parse_known_hosts_line(line, index + 1))
        .collect()
}

fn parse_known_hosts_line(line: &str, number: usize) -> Option<KnownHostEntry> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let mut fields = trimmed.split_whitespace();
    let mut first = fields.next()?;
    let marker = match first {
        "@cert-authority" => KnownHostMarker::CertAuthority,
        "@revoked" => KnownHostMarker::Revoked,
        other if other.starts_with('@') => return None,
        _ => KnownHostMarker::None,
    };
    if marker != KnownHostMarker::None {
        first = fields.next()?;
    }
    let hosts = parse_host_field(first)?;
    let declared_type = fields.next()?;
    let key = KNOWN_HOSTS_BASE64.decode(fields.next()?).ok()?;
    let key_type = key_blob_type(&key)?;
    if key_type != declared_type {
        return None;
    }
    Some(KnownHostEntry {
        marker,
        hosts,
        key_type,
        key,
        line: number,
    })
}

fn parse_host_field(field: &str) -> Option<HostField> {
    if let Some(rest) = field.strip_prefix("|1|") {
        let (salt, hash) = rest.split_once('|')?;
        return Some(HostField::Hashed {
            salt: KNOWN_HOSTS_BASE64.decode(salt).ok()?,
            hash: KNOWN_HOSTS_BASE64.decode(hash).ok()?,
        });
    }
    let patterns: Vec<String> = field
        .split(',')
        .filter(|pattern| !pattern.is_empty())
        .map(str::to_string)
        .collect();
    (!patterns.is_empty()).then_some(HostField::Patterns(patterns))
}

/// The key type an SSH public key blob starts with (an SSH `string`).
fn key_blob_type(blob: &[u8]) -> Option<String> {
    let length_bytes: [u8; 4] = blob.get(..4)?.try_into().ok()?;
    let length = usize::try_from(u32::from_be_bytes(length_bytes)).ok()?;
    let name = blob.get(4..4usize.checked_add(length)?)?;
    let name = std::str::from_utf8(name).ok()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// The name OpenSSH looks a host up under in known_hosts
/// (`get_hostfile_hostname_ipaddr`): the `HostKeyAlias` verbatim when set,
/// otherwise the lowercased host name, as `[host]:port` for a port other
/// than 22. The same name applies to a `ProxyCommand` transport, where `host`
/// is the configured `HostName`.
pub fn known_hosts_lookup_name(host: &str, port: u16, host_key_alias: Option<&str>) -> String {
    if let Some(alias) = host_key_alias.filter(|alias| !alias.is_empty()) {
        return alias.to_string();
    }
    let host = host.to_lowercase();
    if port == DEFAULT_SSH_PORT || port == 0 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

// ---------------------------------------------------------------------------
// The key the server presented
// ---------------------------------------------------------------------------

/// A host key as received in the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedKey {
    blob: Vec<u8>,
    key_type: String,
}

impl PresentedKey {
    pub fn from_blob(blob: &[u8]) -> Result<Self, DbError> {
        let key_type = key_blob_type(blob).ok_or_else(|| {
            DbError::connection_failed("SSH server presented a malformed host key")
        })?;
        Ok(Self {
            blob: blob.to_vec(),
            key_type,
        })
    }

    pub fn key_type(&self) -> &str {
        &self.key_type
    }

    /// `SHA256:<base64>`, as `ssh-keygen -lf` and ssh's prompt print it.
    pub fn fingerprint(&self) -> String {
        sha256_base64_fingerprint(&self.blob)
    }
}

// ---------------------------------------------------------------------------
// Where keys are looked up
// ---------------------------------------------------------------------------

/// Where DBFlux's own pins live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreLocation {
    /// `<data_dir>/ssh_known_hosts`, resolved only when a key is checked so
    /// a failed dial never touches the data directory.
    DataDir,
    At(PathBuf),
}

impl StoreLocation {
    fn resolve(&self) -> Result<PathBuf, DbError> {
        match self {
            Self::DataDir => tofu_known_hosts_path(),
            Self::At(path) => Ok(path.clone()),
        }
    }
}

/// The files a host key is checked against when the SSH config does not name
/// others (`UserKnownHostsFile` / `GlobalKnownHostsFile`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyPaths {
    pub default_user_files: Vec<PathBuf>,
    pub default_global_files: Vec<PathBuf>,
    pub store: StoreLocation,
}

impl HostKeyPaths {
    /// OpenSSH's defaults for this user and DBFlux's data directory store.
    pub fn system() -> Self {
        let default_user_files = dirs::home_dir()
            .map(|home| {
                let ssh = home.join(".ssh");
                vec![ssh.join("known_hosts"), ssh.join("known_hosts2")]
            })
            .unwrap_or_default();
        Self {
            default_user_files,
            default_global_files: default_global_known_hosts_files(),
            store: StoreLocation::DataDir,
        }
    }
}

#[cfg(not(windows))]
fn default_global_known_hosts_files() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/etc/ssh/ssh_known_hosts"),
        PathBuf::from("/etc/ssh/ssh_known_hosts2"),
    ]
}

/// Win32-OpenSSH keeps the system file under `%ProgramData%\ssh`, which on a
/// machine without the OpenSSH server any local user can create. DBFlux does
/// not check Windows ACLs, so that default is not read; a
/// `GlobalKnownHostsFile` the SSH config names still is.
#[cfg(windows)]
fn default_global_known_hosts_files() -> Vec<PathBuf> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// Policy for keys nobody knows yet
// ---------------------------------------------------------------------------

static ACCEPT_NEW_HOST_KEYS: AtomicBool = AtomicBool::new(false);

/// Applies the "trust new SSH host keys without asking" setting to every
/// tunnel this process opens from now on. Off unless the user turns it on.
pub fn set_accept_new_host_keys(enabled: bool) {
    ACCEPT_NEW_HOST_KEYS.store(enabled, Ordering::SeqCst);
}

pub(crate) fn accept_new_host_keys() -> bool {
    ACCEPT_NEW_HOST_KEYS.load(Ordering::SeqCst)
}

/// What happens to a host key no source knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownHostKeyPolicy {
    /// Refuse, and let the GUI ask the user to trust it.
    Confirm,
    /// Pin it in DBFlux's store and connect, unless known_hosts holds a key
    /// of another type for the host: then it is handled as `Confirm`.
    AcceptNew,
    /// `StrictHostKeyChecking yes`: refuse without offering to trust it.
    Refuse,
}

/// An explicit `StrictHostKeyChecking` in the SSH config wins over the global
/// setting, because it is the more specific choice for that host.
pub fn unknown_host_key_policy(
    strict: Option<StrictHostKeyChecking>,
    accept_new_setting: bool,
) -> UnknownHostKeyPolicy {
    match strict {
        Some(StrictHostKeyChecking::Yes) => UnknownHostKeyPolicy::Refuse,
        Some(StrictHostKeyChecking::AcceptNew | StrictHostKeyChecking::No) => {
            UnknownHostKeyPolicy::AcceptNew
        }
        Some(StrictHostKeyChecking::Ask) => UnknownHostKeyPolicy::Confirm,
        None if accept_new_setting => UnknownHostKeyPolicy::AcceptNew,
        None => UnknownHostKeyPolicy::Confirm,
    }
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// What the known sources say about a presented key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    Trusted,
    /// Listed as `@revoked`; the location is `file:line`.
    Revoked {
        location: String,
    },
    /// A different key of the same type is recorded at these locations.
    Changed {
        expected_in: Vec<String>,
    },
    /// No source knows a key of this type for the host. `other_key_types`
    /// are types known_hosts or DBFlux's pins hold for it, which the server
    /// did not use.
    Unknown {
        other_key_types: Vec<String>,
    },
}

/// An entry that records a different key of the presented type, and how
/// to remove it.
struct Conflict {
    location: String,
    removal: String,
}

#[derive(Default)]
struct Findings {
    revoked: Option<String>,
    matched: bool,
    mismatched: Vec<Conflict>,
    other_types: BTreeSet<String>,
    known_types: BTreeSet<String>,
    /// Fingerprint and type of every key known_hosts records for the host,
    /// to tell what type an untyped DBFlux pin has.
    recorded: Vec<(String, String)>,
}

impl Findings {
    fn scan(
        &mut self,
        entries: &[KnownHostEntry],
        path: &Path,
        global: bool,
        name: &str,
        key: &PresentedKey,
    ) {
        for entry in entries.iter().filter(|entry| entry.matches_host(name)) {
            let location = format!("{}:{}", path.display(), entry.line);
            match entry.marker {
                KnownHostMarker::Revoked => {
                    if entry.key == key.blob && self.revoked.is_none() {
                        self.revoked = Some(location);
                    }
                }
                KnownHostMarker::CertAuthority => {}
                KnownHostMarker::None => {
                    self.known_types.insert(entry.key_type.clone());
                    self.recorded.push((
                        sha256_base64_fingerprint(&entry.key),
                        entry.key_type.clone(),
                    ));
                    if entry.key_type != key.key_type {
                        self.other_types.insert(entry.key_type.clone());
                    } else if entry.key == key.blob {
                        self.matched = true;
                    } else {
                        let removal = known_hosts_removal(entry, path, &location, global, name);
                        self.mismatched.push(Conflict { location, removal });
                    }
                }
            }
        }
    }
}

/// Checks host keys for one dial target. Every SSH transport goes through
/// [`HostKeyVerifier::verify_session`] before authentication; see
/// `open_verified_session` in the crate root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyVerifier {
    /// `host:port` as dialled, for messages.
    display: String,
    lookup_name: String,
    store_host: String,
    store_port: u16,
    user_files: Vec<PathBuf>,
    global_files: Vec<PathBuf>,
    store: StoreLocation,
    policy: UnknownHostKeyPolicy,
}

impl HostKeyVerifier {
    /// `host`/`port` are the target as dialled (the `HostName` and `Port`
    /// for a ProxyCommand transport too, like OpenSSH).
    pub fn new(
        host: &str,
        port: u16,
        options: &HostKeyOptions,
        paths: &HostKeyPaths,
        accept_new_setting: bool,
    ) -> Self {
        let alias = options
            .host_key_alias
            .as_deref()
            .filter(|alias| !alias.is_empty());
        // The pin key stays `host\tport` without an alias, so pins written
        // by earlier versions keep matching. With an alias, the pin follows
        // the alias like OpenSSH's known_hosts entry does, which keeps hosts
        // behind one ProxyCommand apart.
        let (store_host, store_port) = match alias {
            Some(alias) => (alias.to_string(), DEFAULT_SSH_PORT),
            None => (host.to_string(), port),
        };
        let display = match alias {
            Some(alias) => format!("{host}:{port} (HostKeyAlias {alias})"),
            None => format!("{host}:{port}"),
        };
        Self {
            display,
            lookup_name: known_hosts_lookup_name(host, port, alias),
            store_host,
            store_port,
            user_files: options
                .user_known_hosts_files
                .clone()
                .unwrap_or_else(|| paths.default_user_files.clone()),
            global_files: options
                .global_known_hosts_files
                .clone()
                .unwrap_or_else(|| paths.default_global_files.clone()),
            store: paths.store.clone(),
            policy: unknown_host_key_policy(options.strict_host_key_checking, accept_new_setting),
        }
    }

    pub fn lookup_name(&self) -> &str {
        &self.lookup_name
    }

    pub fn policy(&self) -> UnknownHostKeyPolicy {
        self.policy
    }

    pub fn known_hosts_files(&self) -> impl Iterator<Item = &Path> {
        self.user_files
            .iter()
            .chain(&self.global_files)
            .map(PathBuf::as_path)
    }

    fn read_entries(&self) -> Vec<(PathBuf, bool, Vec<KnownHostEntry>)> {
        let user = self.user_files.iter().map(|path| (path, false));
        let global = self.global_files.iter().map(|path| (path, true));
        user.chain(global)
            .filter_map(
                |(path, global)| match read_trusted_known_hosts(path, global) {
                    Ok(Some(text)) => Some((path.clone(), global, parse_known_hosts(&text))),
                    Ok(None) => None,
                    Err(reason) => {
                        log::warn!(
                            "[SSH] Skipping known_hosts file {}: {reason}",
                            path.display()
                        );
                        None
                    }
                },
            )
            .collect()
    }

    fn store_entry_key(&self) -> String {
        format!("{}\t{}", self.store_host, self.store_port)
    }

    /// Every source must agree: a key that differs from any key of the same
    /// type recorded for the host, in any known_hosts file or in DBFlux's
    /// store, is a change, even when another source matches it. A stale or
    /// planted entry is then noticed instead of being outvoted, and the
    /// message names every entry that disagrees. With no disagreement, one
    /// matching source is enough. Keys of other types never conflict.
    pub fn check(&self, key: &PresentedKey) -> Result<HostKeyStatus, DbError> {
        Ok(self.evaluate(key)?.0)
    }

    /// [`Self::check`] plus how to remove each disagreeing entry.
    fn evaluate(&self, key: &PresentedKey) -> Result<(HostKeyStatus, Vec<String>), DbError> {
        let mut findings = Findings::default();
        for (path, global, entries) in self.read_entries() {
            findings.scan(&entries, &path, global, &self.lookup_name, key);
        }

        if let Some(location) = findings.revoked {
            return Ok((HostKeyStatus::Revoked { location }, Vec::new()));
        }

        let store_path = self.store.resolve()?;
        let mut all_pins = load_tofu_known_hosts(&store_path)?;
        let entry_key = self.store_entry_key();
        let mut pins = all_pins
            .get(&entry_key)
            .map(|value| parse_pins(value))
            .unwrap_or_default();
        let mut store_matched = false;
        let mut store_conflict = None;
        let mut rewrite_pins = false;
        for pin in &mut pins {
            if pin.fingerprint == key.fingerprint() {
                store_matched = true;
                // A legacy pin (hex, or no type) is rewritten with its type.
                if pin.legacy_hex || pin.key_type.is_none() {
                    pin.legacy_hex = false;
                    pin.key_type = Some(key.key_type.clone());
                    rewrite_pins = true;
                }
                continue;
            }
            let pin_type = pin.key_type.clone().or_else(|| {
                findings
                    .recorded
                    .iter()
                    .find(|(fingerprint, _)| fingerprint == &pin.fingerprint)
                    .map(|(_, key_type)| key_type.clone())
            });
            match pin_type {
                Some(pin_type) if pin_type == key.key_type => {
                    store_conflict = Some(false);
                }
                Some(pin_type) => {
                    findings.other_types.insert(pin_type);
                }
                // An older pin records no key type. When known_hosts vouches
                // for the presented key it is taken as a key of another type
                // (libssh2 used to pick ECDSA first); otherwise it is the
                // only record and may be this type, so it still fails.
                None if findings.matched => {
                    log::warn!(
                        "[SSH] Ignoring a DBFlux pin of unknown key type for {}: known_hosts \
                         vouches for the presented {} key",
                        self.display,
                        key.key_type
                    );
                }
                None => store_conflict = Some(true),
            }
        }

        let mut conflicts = findings.mismatched;
        if let Some(untyped) = store_conflict {
            conflicts.push(Conflict {
                location: store_path.display().to_string(),
                removal: self.store_removal(&store_path, untyped),
            });
        }
        if !conflicts.is_empty() {
            let (locations, removals) = conflicts
                .into_iter()
                .map(|conflict| (conflict.location, conflict.removal))
                .unzip();
            return Ok((
                HostKeyStatus::Changed {
                    expected_in: locations,
                },
                removals,
            ));
        }

        if store_matched && rewrite_pins {
            all_pins.insert(entry_key, format_pins(&pins));
            save_tofu_known_hosts(&store_path, &all_pins)?;
        }
        if findings.matched || store_matched {
            return Ok((HostKeyStatus::Trusted, Vec::new()));
        }
        Ok((
            HostKeyStatus::Unknown {
                other_key_types: findings.other_types.into_iter().collect(),
            },
            Vec::new(),
        ))
    }

    fn store_removal(&self, store_path: &Path, untyped: bool) -> String {
        let note = if untyped {
            " (an older pin whose key type DBFlux did not record)"
        } else {
            ""
        };
        format!(
            "delete DBFlux's pin{note}, the line starting with `{}` (a tab between name \
             and port), from {}",
            self.store_entry_key(),
            store_path.display()
        )
    }

    /// Applies [`Self::check`] and the unknown-key policy.
    pub fn verify_presented(&self, key: &PresentedKey) -> Result<(), DbError> {
        let (status, removals) = self.evaluate(key)?;
        match status {
            HostKeyStatus::Trusted => Ok(()),
            HostKeyStatus::Revoked { location } => Err(DbError::connection_failed(format!(
                "The SSH host key of {} ({} key {}) is marked @revoked in {location}. DBFlux \
                 did not connect.",
                self.display,
                key.key_type,
                key.fingerprint()
            ))),
            HostKeyStatus::Changed { expected_in } => Err(DbError::connection_failed(
                self.changed_message(key, &expected_in, &removals),
            )),
            HostKeyStatus::Unknown { other_key_types } => match self.policy {
                // known_hosts vouches for this host with another key type, so
                // a new type is what an interceptor offering only that type
                // would present: never pin it unasked.
                UnknownHostKeyPolicy::AcceptNew if other_key_types.is_empty() => {
                    self.pin(key)?;
                    log::warn!(
                        "[SSH] Trusted the new host key of {} ({} {}) without asking, as \
                         configured",
                        self.display,
                        key.key_type,
                        key.fingerprint()
                    );
                    Ok(())
                }
                UnknownHostKeyPolicy::Refuse => Err(DbError::connection_failed(format!(
                    "The SSH host key of {} is not known ({} key {}) and the SSH config sets \
                     StrictHostKeyChecking yes for it (or a value DBFlux could not read, which \
                     counts as yes), so DBFlux did not connect. Add the key \
                     to your known_hosts file, for example by connecting once with ssh, and \
                     try again.",
                    self.display,
                    key.key_type,
                    key.fingerprint()
                ))),
                UnknownHostKeyPolicy::Confirm | UnknownHostKeyPolicy::AcceptNew => {
                    let unknown = UnknownHostKey {
                        host: self.display.clone(),
                        key_type: key.key_type.clone(),
                        fingerprint: key.fingerprint(),
                        other_key_types,
                        store: self.store.clone(),
                        store_host: self.store_host.clone(),
                        store_port: self.store_port,
                    };
                    let message = unknown.refusal_message();
                    remember_pending(unknown);
                    Err(DbError::connection_failed(message))
                }
            },
        }
    }

    fn changed_message(
        &self,
        key: &PresentedKey,
        expected_in: &[String],
        removals: &[String],
    ) -> String {
        format!(
            "WARNING: THE SSH HOST KEY OF {} HAS CHANGED. Someone may be intercepting this \
             connection (a man-in-the-middle attack), or the server's key was replaced. The \
             server presented a {} key with fingerprint {}, which does not match the key \
             recorded in {}. DBFlux did not connect. If the key was changed on purpose, \
             confirm the new fingerprint with the server's administrator, then remove each old \
             entry: {}.",
            self.display,
            key.key_type,
            key.fingerprint(),
            expected_in.join(", "),
            removals.join("; "),
        )
    }

    fn pin(&self, key: &PresentedKey) -> Result<(), DbError> {
        pin_fingerprint(
            &self.store,
            &self.store_host,
            self.store_port,
            &key.fingerprint(),
            &key.key_type,
        )
    }

    /// Asks libssh2 for the key types known_hosts already records for this
    /// host first, like OpenSSH orders `HostKeyAlgorithms`. Without this a
    /// server offering several types could present one known_hosts lacks
    /// and the user would be asked about a host ssh already trusts.
    pub(crate) fn prefer_known_key_types(&self, session: &Session) {
        let mut known = BTreeSet::new();
        for (_, _, entries) in self.read_entries() {
            for entry in entries.iter().filter(|entry| {
                entry.marker == KnownHostMarker::None && entry.matches_host(&self.lookup_name)
            }) {
                known.insert(entry.key_type.clone());
            }
        }
        let supported = match session.supported_algs(MethodType::HostKey) {
            Ok(supported) => supported,
            Err(error) => {
                log::warn!("[SSH] Could not list host key algorithms: {error}");
                return;
            }
        };
        let Some(preference) = host_key_algorithm_preference(&supported, &known) else {
            return;
        };
        if let Err(error) = session.method_pref(MethodType::HostKey, &preference) {
            log::warn!("[SSH] Could not set the host key algorithm preference: {error}");
        }
    }

    /// The only constructor of [`VerifiedSession`]: a session that went
    /// through the handshake and whose host key passed [`Self::verify_presented`].
    pub(crate) fn verify_session(&self, session: Session) -> Result<VerifiedSession, DbError> {
        let (blob, _) = session.host_key().ok_or_else(|| {
            DbError::connection_failed("SSH server did not present a host key".to_string())
        })?;
        let key = PresentedKey::from_blob(blob)?;
        self.verify_presented(&key)?;
        Ok(VerifiedSession { session })
    }
}

/// Reads a known_hosts file that may vouch for host keys: `None` when it does
/// not exist. The checks run on the opened handle, so the file read is the
/// file checked.
fn read_trusted_known_hosts(path: &Path, global: bool) -> Result<Option<String>, String> {
    use std::io::Read as _;

    // Opening a FIFO for reading blocks, so only a regular file is opened;
    // the authoritative checks below then run on the opened handle.
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err("it is not a regular file".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    known_hosts_file_is_trusted(&metadata, global)?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(Some(text))
}

/// A file that vouches for host keys must not be one another local user can
/// change, or that user could plant a key and intercept the tunnel without a
/// prompt. Every known_hosts file must be owned by root or by the owner of the
/// home directory (the user DBFlux runs for) and not be writable by everyone.
/// A global file must not be group-writable either; a user file may be, since
/// a umask of 002 with a group per user makes that common and harmless.
#[cfg(unix)]
fn known_hosts_file_is_trusted(metadata: &std::fs::Metadata, global: bool) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    if !metadata.is_file() {
        return Err("it is not a regular file".to_string());
    }
    let mode = metadata.mode();
    if mode & 0o002 != 0 {
        return Err("it is writable by every user".to_string());
    }
    if global && mode & 0o020 != 0 {
        return Err("it is writable by its group".to_string());
    }
    let owner = metadata.uid();
    let home_owner = dirs::home_dir()
        .and_then(|home| std::fs::metadata(home).ok())
        .map(|home| home.uid());
    if owner == 0 || Some(owner) == home_owner {
        Ok(())
    } else {
        Err(format!("it is owned by another user (uid {owner})"))
    }
}

/// Windows ACLs are not checked; the default `%ProgramData%` file is not read
/// at all (see `default_global_known_hosts_files`).
#[cfg(not(unix))]
fn known_hosts_file_is_trusted(metadata: &std::fs::Metadata, _global: bool) -> Result<(), String> {
    if metadata.is_file() {
        Ok(())
    } else {
        Err("it is not a regular file".to_string())
    }
}

/// `name` as one POSIX shell word, for a command the user may paste.
fn shell_quote(name: &str) -> String {
    format!("'{}'", name.replace('\'', "'\\''"))
}

/// How to remove a known_hosts entry that disagrees. `ssh-keygen -R`
/// without `-f` only edits `~/.ssh/known_hosts`, so the command names the
/// file; a line with patterns or several names cannot be removed by name
/// without touching other hosts, so it is named for editing by hand.
fn known_hosts_removal(
    entry: &KnownHostEntry,
    path: &Path,
    location: &str,
    global: bool,
    name: &str,
) -> String {
    let by_name = match &entry.hosts {
        HostField::Hashed { .. } => true,
        HostField::Patterns(patterns) => {
            patterns.len() == 1
                && patterns
                    .iter()
                    .all(|pattern| !pattern.contains(['*', '?', '!']))
        }
    };
    if !by_name {
        return format!("edit {location} by hand (a pattern or several hosts on one line)");
    }
    let command = keygen_remove_command(path, name, cfg!(unix));
    if global {
        format!("{command} as root")
    } else {
        command
    }
}

/// The `ssh-keygen -f FILE -R NAME` command to suggest. A POSIX shell gets
/// both quoted. Windows shells quote differently (cmd.exe passes `'`
/// through), so there the command is spelled out only when neither needs
/// quoting, and otherwise described.
fn keygen_remove_command(path: &Path, name: &str, posix: bool) -> String {
    let file = path.display().to_string();
    if posix {
        return format!(
            "run `ssh-keygen -f {} -R {}`",
            shell_quote(&file),
            shell_quote(name)
        );
    }
    let plain = |text: &str, extra: &[char]| {
        !text.is_empty()
            && text.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '.' | '-' | '_' | ':' | '[' | ']')
                    || extra.contains(&c)
            })
    };
    if plain(name, &[]) && plain(&file, &['\\']) {
        format!("run `ssh-keygen -f {file} -R {name}`")
    } else {
        format!("run `ssh-keygen -f` with {file} and `-R` with the entry's name")
    }
}

/// One key DBFlux pinned for a host. Pins written by this version carry the
/// key type; older ones are a bare `SHA256:` fingerprint (type unknown) or
/// the hex of the whole key blob (type read from the blob).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pin {
    fingerprint: String,
    key_type: Option<String>,
    legacy_hex: bool,
}

/// Placeholder written for a pin whose key type is not known.
const UNKNOWN_PIN_TYPE: &str = "-";

/// Parses the value of a pin line: `fingerprint\ttype` pairs, or a single
/// legacy fingerprint or hex blob.
fn parse_pins(value: &str) -> Vec<Pin> {
    let fields: Vec<&str> = value.split('\t').collect();
    if let [single] = fields.as_slice() {
        if single.starts_with("SHA256:") {
            return vec![Pin {
                fingerprint: (*single).to_string(),
                key_type: None,
                legacy_hex: false,
            }];
        }
        return match hex_decode(single) {
            Some(blob) => vec![Pin {
                fingerprint: sha256_base64_fingerprint(&blob),
                key_type: key_blob_type(&blob),
                legacy_hex: true,
            }],
            // Unreadable: kept, never matches, type unknown.
            None => vec![Pin {
                fingerprint: (*single).to_string(),
                key_type: None,
                legacy_hex: false,
            }],
        };
    }
    fields
        .chunks(2)
        .filter_map(|pair| match pair {
            [fingerprint, key_type] => Some(Pin {
                fingerprint: (*fingerprint).to_string(),
                key_type: (*key_type != UNKNOWN_PIN_TYPE).then(|| (*key_type).to_string()),
                legacy_hex: false,
            }),
            _ => None,
        })
        .collect()
}

fn format_pins(pins: &[Pin]) -> String {
    pins.iter()
        .map(|pin| {
            format!(
                "{}\t{}",
                pin.fingerprint,
                pin.key_type.as_deref().unwrap_or(UNKNOWN_PIN_TYPE)
            )
        })
        .collect::<Vec<_>>()
        .join("\t")
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || text.is_empty() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            text.get(index..index + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect()
}

/// The algorithms for `known_types` first, in libssh2's own order, then the
/// rest. Host certificate algorithms are dropped once known_hosts has keys
/// for the host, since DBFlux cannot check a certificate against them.
/// `None` keeps libssh2's default when known_hosts knows nothing.
pub fn host_key_algorithm_preference(
    supported: &[&str],
    known_types: &BTreeSet<String>,
) -> Option<String> {
    if known_types.is_empty() {
        return None;
    }
    let plain = supported
        .iter()
        .copied()
        .filter(|algorithm| !algorithm.contains("-cert-"));
    let (first, rest): (Vec<&str>, Vec<&str>) =
        plain.partition(|algorithm| known_types.contains(algorithm_key_type(algorithm)));
    if first.is_empty() {
        return None;
    }
    Some(first.into_iter().chain(rest).collect::<Vec<_>>().join(","))
}

/// RSA keys are stored as `ssh-rsa` whatever signature algorithm signs with them.
fn algorithm_key_type(algorithm: &str) -> &str {
    match algorithm {
        "rsa-sha2-256" | "rsa-sha2-512" => "ssh-rsa",
        other => other,
    }
}

/// An SSH session whose host key was verified. Authentication and tunnels
/// only take this type, so no transport can skip the host key check.
pub struct VerifiedSession {
    session: Session,
}

impl VerifiedSession {
    pub(crate) fn session(&self) -> &Session {
        &self.session
    }

    pub(crate) fn into_session(self) -> Session {
        self.session
    }
}

// ---------------------------------------------------------------------------
// Unknown keys waiting for the user
// ---------------------------------------------------------------------------

/// A host key a connect refused because nobody knows it yet. The GUI shows
/// it for confirmation and calls [`UnknownHostKey::trust`] on "Trust and
/// remember".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownHostKey {
    /// `host:port` as dialled, plus the HostKeyAlias when one applies.
    pub host: String,
    pub key_type: String,
    /// `SHA256:<base64>`.
    pub fingerprint: String,
    /// Key types known_hosts records for the host that the server did not
    /// offer; worth a warning in the prompt.
    pub other_key_types: Vec<String>,
    store: StoreLocation,
    store_host: String,
    store_port: u16,
}

/// The text every unknown-key refusal starts with.
const UNKNOWN_HOST_KEY_LEAD: &str = "Unknown SSH host key for ";

impl UnknownHostKey {
    /// Names both the host and the key, so a refusal is matched to the
    /// pending entry of that host: hosts behind one proxy, cloned machines or
    /// an interceptor can present the same key for several hosts.
    fn refusal_header(&self) -> String {
        format!(
            "{UNKNOWN_HOST_KEY_LEAD}{} ({} key {})",
            self.host, self.key_type, self.fingerprint
        )
    }

    fn refusal_message(&self) -> String {
        format!(
            "{}. DBFlux did not connect. Connect to it once from the DBFlux window to \
             review and trust the key, or add it to your known_hosts file.",
            self.refusal_header()
        )
    }

    /// Pins the key in DBFlux's store, never in the user's known_hosts.
    pub fn trust(&self) -> Result<(), DbError> {
        pin_fingerprint(
            &self.store,
            &self.store_host,
            self.store_port,
            &self.fingerprint,
            &self.key_type,
        )?;
        forget_pending(self);
        Ok(())
    }

    /// The user declined to trust the key; it is no longer offered.
    pub fn dismiss(&self) {
        forget_pending(self);
    }

    /// Whether the key still waits for an answer. A queued prompt for the
    /// same host and key is settled once that key was trusted or dismissed.
    pub fn is_pending(&self) -> bool {
        pending().iter().any(|existing| existing == self)
    }
}

const MAX_PENDING: usize = 16;

static PENDING: Mutex<Vec<UnknownHostKey>> = Mutex::new(Vec::new());

fn pending() -> std::sync::MutexGuard<'static, Vec<UnknownHostKey>> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn remember_pending(key: UnknownHostKey) {
    let mut pending = pending();
    pending.retain(|existing| existing != &key);
    pending.push(key);
    let excess = pending.len().saturating_sub(MAX_PENDING);
    pending.drain(..excess);
}

fn forget_pending(key: &UnknownHostKey) {
    pending().retain(|existing| existing != key);
}

/// The unknown host key a connect error was refused for, if any.
///
/// Errors reach the GUI as text (often through `{:?}`), so the refusal is
/// matched by its host and fingerprint against keys this process actually
/// refused; text that merely looks like a refusal never produces a prompt.
pub fn unknown_host_key_in_error(message: &str) -> Option<UnknownHostKey> {
    if !message.contains(UNKNOWN_HOST_KEY_LEAD) {
        return None;
    }
    pending()
        .iter()
        .rev()
        .find(|key| message.contains(&key.refusal_header()))
        .cloned()
}

fn pin_fingerprint(
    store: &StoreLocation,
    host: &str,
    port: u16,
    fingerprint: &str,
    key_type: &str,
) -> Result<(), DbError> {
    let path = store.resolve()?;
    let mut all_pins = load_tofu_known_hosts(&path)?;
    let entry_key = format!("{host}\t{port}");
    let mut pins = all_pins
        .get(&entry_key)
        .map(|value| parse_pins(value))
        .unwrap_or_default();
    if pins.iter().any(|pin| pin.fingerprint == fingerprint) {
        return Ok(());
    }
    if pins
        .iter()
        .any(|pin| pin.key_type.as_deref() == Some(key_type))
    {
        // Trusting never replaces a pin of the same type: a changed key goes
        // through the mismatch error and a deliberate removal.
        return Err(DbError::connection_failed(format!(
            "DBFlux already has a different {key_type} host key recorded for {host}:{port} in \
             {}; remove that pin first if the key was changed on purpose.",
            path.display()
        )));
    }
    pins.push(Pin {
        fingerprint: fingerprint.to_string(),
        key_type: Some(key_type.to_string()),
        legacy_hex: false,
    });
    all_pins.insert(entry_key, format_pins(&pins));
    save_tofu_known_hosts(&path, &all_pins)
}

#[cfg(test)]
mod tests;
