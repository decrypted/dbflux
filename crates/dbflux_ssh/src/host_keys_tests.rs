#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use std::fs;
use tempfile::TempDir;

/// Public keys made with `ssh-keygen` for these tests only; the fingerprints
/// are what `ssh-keygen -lf` printed for them.
const ED25519_LINE: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPZmwgTjNcoD7TQZR8YGi1umx0lKXa/MVd4iKVCZuORZ";
const ED25519_FINGERPRINT: &str = "SHA256:cwXHzvgvAwqzepKtWJQ8CdBGAoanE25C4WaB8qH7194";
const ECDSA_LINE: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBFtlFBoKSX7m15JzkyDNDCGkfhLka45tmyBDbGXxNQb1Nnm4tFjF9MQO2FqecPbV4Jf/aGte8RgW9t/k7xWErd4=";
const ECDSA_FINGERPRINT: &str = "SHA256:EROfNm+LQ+/KExXt2KYTvsJuzumrPpVVQ3sbqgX0Wjw";

/// `ssh-keygen -H` output for `bastion.example.com` and
/// `[bastion.example.com]:2222`, both with the ed25519 key above.
const HASHED_PORT_22: &str = "|1|iKq4GUaKNei6JoV7oNfl3tyQAg0=|PqSCIENIA1MYZRZqVe1FZn5S6T4=";
const HASHED_PORT_2222: &str = "|1|s76FXn7DmW31Caqt3ot4gvsXxzw=|ufIt6rGrVDM/Ao8NVq/oyevmW4E=";

fn key_from_line(line: &str) -> PresentedKey {
    let blob = KNOWN_HOSTS_BASE64
        .decode(line.split_whitespace().nth(1).unwrap())
        .unwrap();
    PresentedKey::from_blob(&blob).unwrap()
}

fn ed25519() -> PresentedKey {
    key_from_line(ED25519_LINE)
}

fn ecdsa() -> PresentedKey {
    key_from_line(ECDSA_LINE)
}

/// A syntactically valid ed25519 key that is not [`ed25519`].
fn other_ed25519() -> (PresentedKey, String) {
    synthetic_ed25519(7)
}

/// Tests that look a refusal up by fingerprint use their own seed: the
/// pending list is shared by every test in the process.
fn synthetic_ed25519(seed: u8) -> (PresentedKey, String) {
    let mut blob = Vec::new();
    blob.extend_from_slice(&11u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32u32.to_be_bytes());
    blob.extend_from_slice(&[seed; 32]);
    let line = format!(
        "ssh-ed25519 {}",
        base64::engine::general_purpose::STANDARD.encode(&blob)
    );
    (PresentedKey::from_blob(&blob).unwrap(), line)
}

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn known_hosts(&self) -> PathBuf {
        self.dir.path().join("known_hosts")
    }

    fn global(&self) -> PathBuf {
        self.dir.path().join("ssh_known_hosts")
    }

    fn store(&self) -> PathBuf {
        self.dir.path().join("dbflux_ssh_known_hosts")
    }

    fn write_known_hosts(&self, text: &str) {
        fs::write(self.known_hosts(), text).unwrap();
    }

    fn paths(&self) -> HostKeyPaths {
        HostKeyPaths {
            default_user_files: vec![self.known_hosts()],
            default_global_files: vec![self.global()],
            store: StoreLocation::At(self.store()),
        }
    }

    fn verifier(&self, host: &str, port: u16) -> HostKeyVerifier {
        HostKeyVerifier::new(host, port, &HostKeyOptions::default(), &self.paths(), false)
    }

    fn verifier_with(&self, host: &str, port: u16, options: &HostKeyOptions) -> HostKeyVerifier {
        HostKeyVerifier::new(host, port, options, &self.paths(), false)
    }

    fn pins(&self) -> String {
        fs::read_to_string(self.store()).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_plain_hashed_port_and_marker_lines_and_skips_the_rest() {
    let text = format!(
        "# a comment\n\
         \n\
         bastion.example.com,10.0.0.5 {ED25519_LINE} user@laptop\n\
         [db.example.com]:2222 {ECDSA_LINE}\n\
         {HASHED_PORT_22} {ED25519_LINE}\n\
         @revoked * {ECDSA_LINE}\n\
         @cert-authority *.example.com {ED25519_LINE}\n\
         @unknown-marker host {ED25519_LINE}\n\
         broken.example.com ssh-ed25519 !!!not-base64!!!\n\
         liar.example.com ssh-rsa {}\n\
         legacy.example.com 1024 35 1234567890\n\
         \t  # indented comment\n",
        ED25519_LINE.split_whitespace().nth(1).unwrap()
    );

    let entries = parse_known_hosts(&text);

    assert_eq!(entries.len(), 5, "{entries:#?}");
    assert_eq!(entries[0].line, 3);
    assert_eq!(entries[0].marker, KnownHostMarker::None);
    assert_eq!(entries[0].key_type, "ssh-ed25519");
    assert!(entries[0].matches_host("bastion.example.com"));
    assert!(entries[0].matches_host("10.0.0.5"));
    assert!(!entries[0].matches_host("[bastion.example.com]:2222"));

    assert_eq!(entries[1].key_type, "ecdsa-sha2-nistp256");
    assert!(entries[1].matches_host("[db.example.com]:2222"));
    assert!(!entries[1].matches_host("db.example.com"));

    assert_eq!(entries[2].line, 5);
    assert!(entries[2].matches_host("bastion.example.com"));
    assert!(!entries[2].matches_host("other.example.com"));

    assert_eq!(entries[3].marker, KnownHostMarker::Revoked);
    assert!(entries[3].matches_host("anything"));

    // The CA line keeps its own key type and host pattern; it is never read
    // as a host key line for `*.example.com`.
    assert_eq!(entries[4].marker, KnownHostMarker::CertAuthority);
    assert_eq!(entries[4].key_type, "ssh-ed25519");
    assert!(entries[4].matches_host("db.example.com"));
}

#[test]
fn hashed_entries_match_the_name_ssh_keygen_hashed() {
    let entries = parse_known_hosts(&format!(
        "{HASHED_PORT_22} {ED25519_LINE}\n{HASHED_PORT_2222} {ED25519_LINE}\n"
    ));

    assert_eq!(entries.len(), 2);
    assert!(entries[0].matches_host("bastion.example.com"));
    assert!(!entries[0].matches_host("[bastion.example.com]:2222"));
    assert!(entries[1].matches_host("[bastion.example.com]:2222"));
    assert!(!entries[1].matches_host("bastion.example.com"));
}

#[test]
fn patterns_match_globs_case_insensitively_and_honour_negation() {
    let entries = parse_known_hosts(&format!("*.example.com,!secret.example.com {ED25519_LINE}"));
    let entry = &entries[0];

    assert!(entry.matches_host("db.example.com"));
    assert!(entry.matches_host("DB.Example.COM"));
    assert!(!entry.matches_host("secret.example.com"));
    assert!(!entry.matches_host("example.org"));
}

#[test]
fn several_keys_for_one_host_are_all_kept() {
    let (_, other_line) = other_ed25519();
    let entries = parse_known_hosts(&format!(
        "host.example {ED25519_LINE}\nhost.example {ECDSA_LINE}\nhost.example {other_line}\n"
    ));

    assert_eq!(entries.len(), 3);
    assert!(
        entries
            .iter()
            .all(|entry| entry.matches_host("host.example"))
    );
}

#[test]
fn fingerprints_match_ssh_keygen() {
    assert_eq!(ed25519().fingerprint(), ED25519_FINGERPRINT);
    assert_eq!(ecdsa().fingerprint(), ECDSA_FINGERPRINT);
    assert_eq!(ed25519().key_type(), "ssh-ed25519");
}

// ---------------------------------------------------------------------------
// Lookup names
// ---------------------------------------------------------------------------

#[test]
fn lookup_names_follow_openssh() {
    assert_eq!(
        known_hosts_lookup_name("Bastion.Example.com", 22, None),
        "bastion.example.com"
    );
    assert_eq!(
        known_hosts_lookup_name("bastion.example.com", 2222, None),
        "[bastion.example.com]:2222"
    );
    // HostKeyAlias replaces host and port, verbatim.
    assert_eq!(
        known_hosts_lookup_name("10.0.0.5", 2222, Some("Backend-A")),
        "Backend-A"
    );
    assert_eq!(known_hosts_lookup_name("h", 2222, Some("")), "[h]:2222");
}

#[test]
fn a_non_standard_port_is_looked_up_as_host_port_in_plain_and_hashed_files() {
    let fixture = Fixture::new();
    fixture.write_known_hosts(&format!("{HASHED_PORT_2222} {ED25519_LINE}\n"));

    assert_eq!(
        fixture
            .verifier("bastion.example.com", 2222)
            .check(&ed25519())
            .unwrap(),
        HostKeyStatus::Trusted
    );
    // The same host on 22 is a different known_hosts name.
    assert!(matches!(
        fixture
            .verifier("bastion.example.com", 22)
            .check(&ed25519())
            .unwrap(),
        HostKeyStatus::Unknown { .. }
    ));
}

#[test]
fn host_key_alias_is_the_lookup_name_and_the_pin_name() {
    let fixture = Fixture::new();
    fixture.write_known_hosts(&format!("backend-a {ED25519_LINE}\n"));
    let alias = HostKeyOptions {
        host_key_alias: Some("backend-a".to_string()),
        ..HostKeyOptions::default()
    };

    let verifier = fixture.verifier_with("proxy.internal", 2200, &alias);
    assert_eq!(verifier.lookup_name(), "backend-a");
    assert_eq!(verifier.check(&ed25519()).unwrap(), HostKeyStatus::Trusted);

    // Two backends that share a HostName behind one proxy get separate pins.
    let accept_new = HostKeyOptions {
        host_key_alias: Some("backend-b".to_string()),
        strict_host_key_checking: Some(StrictHostKeyChecking::AcceptNew),
        ..HostKeyOptions::default()
    };
    fixture
        .verifier_with("proxy.internal", 2200, &accept_new)
        .verify_presented(&ecdsa())
        .unwrap();
    assert_eq!(
        fixture.pins(),
        format!("backend-b\t22\t{ECDSA_FINGERPRINT}\n")
    );
}

#[test]
fn ssh_config_files_replace_the_default_known_hosts_files() {
    let fixture = Fixture::new();
    fixture.write_known_hosts(&format!("host.example {ED25519_LINE}\n"));
    let custom = fixture.dir.path().join("custom_known_hosts");
    fs::write(&custom, format!("host.example {ECDSA_LINE}\n")).unwrap();
    let options = HostKeyOptions {
        user_known_hosts_files: Some(vec![custom.clone()]),
        ..HostKeyOptions::default()
    };

    let verifier = fixture.verifier_with("host.example", 22, &options);

    assert_eq!(
        verifier.known_hosts_files().collect::<Vec<_>>(),
        vec![custom.as_path(), fixture.global().as_path()]
    );
    assert_eq!(verifier.check(&ecdsa()).unwrap(), HostKeyStatus::Trusted);
    assert!(matches!(
        verifier.check(&ed25519()).unwrap(),
        HostKeyStatus::Unknown { .. }
    ));
}

// ---------------------------------------------------------------------------
// Decisions
// ---------------------------------------------------------------------------

#[test]
fn a_matching_known_hosts_key_is_trusted_without_touching_the_store() {
    let fixture = Fixture::new();
    fixture.write_known_hosts(&format!("host.example {ED25519_LINE}\n"));

    fixture
        .verifier("host.example", 22)
        .verify_presented(&ed25519())
        .unwrap();

    assert!(!fixture.store().exists());
}

#[test]
fn the_global_file_counts_too() {
    let fixture = Fixture::new();
    fs::write(fixture.global(), format!("host.example {ED25519_LINE}\n")).unwrap();

    assert_eq!(
        fixture
            .verifier("host.example", 22)
            .check(&ed25519())
            .unwrap(),
        HostKeyStatus::Trusted
    );
}

#[test]
fn a_different_key_of_the_same_type_is_a_change_with_the_fingerprint() {
    let fixture = Fixture::new();
    fixture.write_known_hosts(&format!("# x\nhost.example {ED25519_LINE}\n"));
    let (attacker, _) = other_ed25519();

    let verifier = fixture.verifier("host.example", 22);
    assert_eq!(
        verifier.check(&attacker).unwrap(),
        HostKeyStatus::Changed {
            expected_in: vec![format!("{}:2", fixture.known_hosts().display())]
        }
    );
    let message = verifier
        .verify_presented(&attacker)
        .unwrap_err()
        .to_string();
    assert!(message.contains("HAS CHANGED"), "{message}");
    assert!(message.contains(&attacker.fingerprint()), "{message}");
    assert!(
        message.contains("ssh-keygen -R 'host.example'"),
        "{message}"
    );
    assert!(unknown_host_key_in_error(&message).is_none());
    assert!(!fixture.store().exists(), "a changed key is never pinned");
}

#[test]
fn any_matching_line_wins_over_an_older_line_like_openssh() {
    let fixture = Fixture::new();
    let (old, old_line) = other_ed25519();
    fixture.write_known_hosts(&format!(
        "host.example {old_line}\nhost.example {ED25519_LINE}\n"
    ));

    let verifier = fixture.verifier("host.example", 22);
    assert_eq!(verifier.check(&ed25519()).unwrap(), HostKeyStatus::Trusted);
    assert_eq!(verifier.check(&old).unwrap(), HostKeyStatus::Trusted);
}

#[test]
fn only_other_key_types_known_is_unknown_and_names_them() {
    let fixture = Fixture::new();
    fixture.write_known_hosts(&format!("host.example {ECDSA_LINE}\n"));

    assert_eq!(
        fixture
            .verifier("host.example", 22)
            .check(&ed25519())
            .unwrap(),
        HostKeyStatus::Unknown {
            other_key_types: vec!["ecdsa-sha2-nistp256".to_string()]
        }
    );
}

#[test]
fn a_revoked_key_fails_even_when_another_line_trusts_it() {
    let fixture = Fixture::new();
    fixture.write_known_hosts(&format!(
        "host.example {ED25519_LINE}\n@revoked * {ED25519_LINE}\n"
    ));

    let verifier = fixture.verifier("host.example", 22);
    assert_eq!(
        verifier.check(&ed25519()).unwrap(),
        HostKeyStatus::Revoked {
            location: format!("{}:2", fixture.known_hosts().display())
        }
    );
    let message = verifier
        .verify_presented(&ed25519())
        .unwrap_err()
        .to_string();
    assert!(message.contains("@revoked"), "{message}");
    assert!(unknown_host_key_in_error(&message).is_none());
}

#[test]
fn a_cert_authority_line_never_vouches_for_a_host_key() {
    let fixture = Fixture::new();
    fixture.write_known_hosts(&format!("@cert-authority host.example {ED25519_LINE}\n"));

    assert_eq!(
        fixture
            .verifier("host.example", 22)
            .check(&ed25519())
            .unwrap(),
        HostKeyStatus::Unknown {
            other_key_types: Vec::new()
        }
    );
}

#[test]
fn the_store_decides_when_known_hosts_is_silent() {
    let fixture = Fixture::new();
    fs::write(
        fixture.store(),
        format!("host.example\t2222\t{ED25519_FINGERPRINT}\n"),
    )
    .unwrap();
    let verifier = fixture.verifier("host.example", 2222);

    assert_eq!(verifier.check(&ed25519()).unwrap(), HostKeyStatus::Trusted);
    assert_eq!(
        verifier.check(&ecdsa()).unwrap(),
        HostKeyStatus::Changed {
            expected_in: vec![fixture.store().display().to_string()]
        }
    );
    let message = verifier.verify_presented(&ecdsa()).unwrap_err().to_string();
    assert!(message.contains("host.example\t2222"), "{message}");
}

#[test]
fn known_hosts_overrules_a_pin_from_a_silent_first_connect() {
    // The pin is what an older DBFlux stored without asking; known_hosts
    // holds the real key.
    let fixture = Fixture::new();
    let (attacker, _) = other_ed25519();
    fixture.write_known_hosts(&format!("host.example {ED25519_LINE}\n"));
    fs::write(
        fixture.store(),
        format!("host.example\t22\t{}\n", attacker.fingerprint()),
    )
    .unwrap();
    let verifier = fixture.verifier("host.example", 22);

    assert!(matches!(
        verifier.check(&attacker).unwrap(),
        HostKeyStatus::Changed { .. }
    ));
    assert_eq!(verifier.check(&ed25519()).unwrap(), HostKeyStatus::Trusted);
}

#[test]
fn a_legacy_hex_pin_still_matches_and_is_upgraded() {
    let fixture = Fixture::new();
    let key = ed25519();
    fs::write(
        fixture.store(),
        format!("host.example\t22\t{}\n", crate::hex_encode(&key.blob)),
    )
    .unwrap();

    assert_eq!(
        fixture.verifier("host.example", 22).check(&key).unwrap(),
        HostKeyStatus::Trusted
    );
    assert_eq!(
        fixture.pins(),
        format!("host.example\t22\t{ED25519_FINGERPRINT}\n")
    );
}

// ---------------------------------------------------------------------------
// Policy for unknown keys
// ---------------------------------------------------------------------------

#[test]
fn policy_follows_strict_host_key_checking_then_the_setting() {
    use StrictHostKeyChecking::*;
    use UnknownHostKeyPolicy as P;

    let cases = [
        (None, false, P::Confirm),
        (None, true, P::AcceptNew),
        (Some(Ask), false, P::Confirm),
        (Some(Ask), true, P::Confirm),
        (Some(Yes), false, P::Refuse),
        (Some(Yes), true, P::Refuse),
        (Some(AcceptNew), false, P::AcceptNew),
        (Some(No), false, P::AcceptNew),
    ];
    for (strict, setting, expected) in cases {
        assert_eq!(
            unknown_host_key_policy(strict, setting),
            expected,
            "{strict:?} with the setting {setting}"
        );
    }
}

#[test]
fn an_unknown_key_is_refused_without_writing_anything_and_says_how_to_trust_it() {
    // This is the path of every non-interactive caller (MCP server,
    // connect on startup, reconnects): the refusal is final for them.
    let fixture = Fixture::new();
    let (key, _) = other_ed25519();

    let message = fixture
        .verifier("fresh.example", 22)
        .verify_presented(&key)
        .unwrap_err()
        .to_string();

    assert!(
        message.contains("Unknown SSH host key for fresh.example:22"),
        "{message}"
    );
    assert!(message.contains(&key.fingerprint()), "{message}");
    assert!(message.contains("ssh-ed25519"), "{message}");
    assert!(message.contains("from the DBFlux window"), "{message}");
    assert!(
        !crate::is_passphrase_required_error_str(&message),
        "must not be mistaken for a passphrase prompt: {message}"
    );
    assert!(!fixture.store().exists(), "nothing is trusted silently");
}

#[test]
fn the_prompt_path_trusts_on_confirm_and_keeps_refusing_on_cancel() {
    let fixture = Fixture::new();
    let key = ecdsa();
    let verifier = fixture.verifier("prompted.example", 2022);

    let first = verifier.verify_presented(&key).unwrap_err().to_string();
    let pending = unknown_host_key_in_error(&first).expect("the GUI can offer this key");
    assert_eq!(pending.host, "prompted.example:2022");
    assert_eq!(pending.key_type, "ecdsa-sha2-nistp256");
    assert_eq!(pending.fingerprint, ECDSA_FINGERPRINT);

    // Cancel: nothing happens, so the next attempt is refused again.
    let again = verifier.verify_presented(&key).unwrap_err().to_string();
    assert!(unknown_host_key_in_error(&again).is_some());
    assert!(!fixture.store().exists());

    // Trust and remember: the key is pinned in DBFlux's store only.
    pending.trust().unwrap();
    assert_eq!(
        fixture.pins(),
        format!("prompted.example\t2022\t{ECDSA_FINGERPRINT}\n")
    );
    assert!(
        !fixture.known_hosts().exists(),
        "known_hosts is never written"
    );
    verifier.verify_presented(&key).unwrap();
    assert!(
        unknown_host_key_in_error(&first).is_none(),
        "no longer pending"
    );

    // Trusting pinned the presented key only; another key is a change.
    let (other, _) = other_ed25519();
    assert!(matches!(
        verifier.check(&other).unwrap(),
        HostKeyStatus::Changed { .. }
    ));
}

#[test]
fn trusting_never_replaces_an_existing_pin() {
    let fixture = Fixture::new();
    let verifier = fixture.verifier("raced.example", 22);
    let (key, _) = synthetic_ed25519(11);
    let message = verifier.verify_presented(&key).unwrap_err().to_string();
    let pending = unknown_host_key_in_error(&message).unwrap();
    fs::write(
        fixture.store(),
        format!("raced.example\t22\t{ECDSA_FINGERPRINT}\n"),
    )
    .unwrap();

    assert!(pending.trust().is_err());
    assert_eq!(
        fixture.pins(),
        format!("raced.example\t22\t{ECDSA_FINGERPRINT}\n")
    );
}

#[test]
fn text_that_only_looks_like_a_refusal_offers_nothing() {
    // A real refusal is pending, so only the fingerprint check stands
    // between forged text and a prompt for that key.
    let fixture = Fixture::new();
    let (key, _) = synthetic_ed25519(14);
    let real = fixture
        .verifier("real.example", 22)
        .verify_presented(&key)
        .unwrap_err()
        .to_string();
    assert!(unknown_host_key_in_error(&real).is_some());

    let (other, _) = synthetic_ed25519(15);
    let forged = format!(
        "Unknown SSH host key for evil.example:22 (ssh-ed25519 key {}).",
        other.fingerprint()
    );
    assert!(unknown_host_key_in_error(&forged).is_none());
    assert!(unknown_host_key_in_error("SSH handshake failed").is_none());
}

#[test]
fn the_refusal_is_recognised_through_debug_formatting() {
    let fixture = Fixture::new();
    let (key, _) = synthetic_ed25519(12);
    let error = fixture
        .verifier("debugged.example", 22)
        .verify_presented(&key)
        .unwrap_err();

    // Two GUI callers report `format!("{:?}", error)`.
    let pending = unknown_host_key_in_error(&format!("{error:?}")).unwrap();
    assert_eq!(pending.host, "debugged.example:22");
}

#[test]
fn accept_new_pins_and_connects_strict_yes_refuses_without_an_offer() {
    let fixture = Fixture::new();
    let accept = HostKeyOptions {
        strict_host_key_checking: Some(StrictHostKeyChecking::AcceptNew),
        ..HostKeyOptions::default()
    };
    fixture
        .verifier_with("new.example", 22, &accept)
        .verify_presented(&ed25519())
        .unwrap();
    assert_eq!(
        fixture.pins(),
        format!("new.example\t22\t{ED25519_FINGERPRINT}\n")
    );

    // accept-new still refuses a changed key.
    let message = fixture
        .verifier_with("new.example", 22, &accept)
        .verify_presented(&ecdsa())
        .unwrap_err()
        .to_string();
    assert!(message.contains("HAS CHANGED"), "{message}");

    let strict = HostKeyOptions {
        strict_host_key_checking: Some(StrictHostKeyChecking::Yes),
        ..HostKeyOptions::default()
    };
    let message = fixture
        .verifier_with("strict.example", 22, &strict)
        .verify_presented(&ed25519())
        .unwrap_err()
        .to_string();
    assert!(message.contains("StrictHostKeyChecking yes"), "{message}");
    assert!(unknown_host_key_in_error(&message).is_none());
}

#[test]
fn the_global_setting_accepts_new_keys_only_when_on() {
    let fixture = Fixture::new();
    let options = HostKeyOptions::default();

    let off = HostKeyVerifier::new("g.example", 22, &options, &fixture.paths(), false);
    assert_eq!(off.policy(), UnknownHostKeyPolicy::Confirm);
    assert!(off.verify_presented(&ed25519()).is_err());

    let on = HostKeyVerifier::new("g.example", 22, &options, &fixture.paths(), true);
    on.verify_presented(&ed25519()).unwrap();
    assert_eq!(
        fixture.pins(),
        format!("g.example\t22\t{ED25519_FINGERPRINT}\n")
    );
}

#[test]
fn the_process_wide_setting_defaults_to_off() {
    assert!(!accept_new_host_keys());
}

#[cfg(unix)]
#[test]
fn the_pin_file_is_written_private() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let accept = HostKeyOptions {
        strict_host_key_checking: Some(StrictHostKeyChecking::AcceptNew),
        ..HostKeyOptions::default()
    };
    fixture
        .verifier_with("mode.example", 22, &accept)
        .verify_presented(&ed25519())
        .unwrap();

    let mode = fs::metadata(fixture.store()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}

#[test]
fn pending_keys_are_capped() {
    let fixture = Fixture::new();
    let (key, _) = synthetic_ed25519(13);
    let first = fixture
        .verifier("cap-0.example", 22)
        .verify_presented(&key)
        .unwrap_err()
        .to_string();
    for index in 1..=MAX_PENDING {
        let _ = fixture
            .verifier(&format!("cap-{index}.example"), 22)
            .verify_presented(&key);
    }

    // Same fingerprint, so the newest refusal is the one offered.
    let offered = unknown_host_key_in_error(&first).unwrap();
    assert_ne!(offered.host, "cap-0.example:22");
    assert!(pending().len() <= MAX_PENDING);
}

// ---------------------------------------------------------------------------
// Host key algorithm preference
// ---------------------------------------------------------------------------

#[test]
fn known_key_types_are_asked_for_first() {
    let supported = [
        "ecdsa-sha2-nistp256",
        "ecdsa-sha2-nistp256-cert-v01@openssh.com",
        "ssh-ed25519",
        "rsa-sha2-512",
        "rsa-sha2-256",
        "ssh-rsa",
    ];
    let known: BTreeSet<String> = ["ssh-ed25519", "ssh-rsa"]
        .into_iter()
        .map(str::to_string)
        .collect();

    assert_eq!(
        host_key_algorithm_preference(&supported, &known).as_deref(),
        Some("ssh-ed25519,rsa-sha2-512,rsa-sha2-256,ssh-rsa,ecdsa-sha2-nistp256")
    );
    assert_eq!(
        host_key_algorithm_preference(&supported, &BTreeSet::new()),
        None
    );
}
