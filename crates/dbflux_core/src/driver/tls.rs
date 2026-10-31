//! Transport-security vocabulary shared by the drivers and the connection form:
//! what each SSL mode of a driver actually guarantees, which one a new
//! connection starts with, and when the form has to warn about it.

use crate::DbError;

/// What a driver's SSL mode guarantees once the connection is open.
///
/// The connection form derives each mode's label and warning from its kind,
/// so a mode cannot claim more protection in the UI than its kind admits. A
/// driver must pick the kind from what its client configuration really does;
/// each driver's tests pin that mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SslModeKind {
    /// No TLS for the session.
    Off,
    /// No TLS for the session; only the login exchange is encrypted, without
    /// checking the server certificate (SQL Server's `off`).
    OffLoginOnly,
    /// TLS when the server offers it, otherwise plaintext, and the
    /// certificate is not checked. Also libpq's `allow`, which the drivers
    /// treat as `prefer`.
    Prefer,
    /// TLS is mandatory, the certificate is not checked.
    RequiredUnverified,
    /// TLS is mandatory, the certificate chain is checked but not the
    /// hostname.
    VerifyCa,
    /// TLS is mandatory, the certificate chain and the hostname are checked.
    VerifyFull,
}

impl SslModeKind {
    /// Every kind, for tests that check each one has an honest label.
    pub const ALL: [SslModeKind; 6] = [
        SslModeKind::Off,
        SslModeKind::OffLoginOnly,
        SslModeKind::Prefer,
        SslModeKind::RequiredUnverified,
        SslModeKind::VerifyCa,
        SslModeKind::VerifyFull,
    ];

    /// The i18n key of the mode's label in the connection form.
    pub fn label_key(self) -> &'static str {
        match self {
            SslModeKind::Off => "connection_manager.ssl_mode.option.off",
            SslModeKind::OffLoginOnly => "connection_manager.ssl_mode.option.off_login_only",
            SslModeKind::Prefer => "connection_manager.ssl_mode.option.prefer",
            SslModeKind::RequiredUnverified => {
                "connection_manager.ssl_mode.option.required_unverified"
            }
            SslModeKind::VerifyCa => "connection_manager.ssl_mode.option.verify_ca",
            SslModeKind::VerifyFull => "connection_manager.ssl_mode.option.verify_full",
        }
    }

    /// Whether the whole session is guaranteed to be encrypted.
    pub fn always_encrypts(self) -> bool {
        matches!(
            self,
            SslModeKind::RequiredUnverified | SslModeKind::VerifyCa | SslModeKind::VerifyFull
        )
    }

    /// Whether the server certificate chain is checked.
    pub fn verifies_certificate(self) -> bool {
        matches!(self, SslModeKind::VerifyCa | SslModeKind::VerifyFull)
    }

    /// Whether the server's hostname is checked against its certificate.
    pub fn verifies_hostname(self) -> bool {
        self == SslModeKind::VerifyFull
    }

    /// The warning the connection form shows for this mode, if any.
    pub fn warning(self) -> Option<TransportWarning> {
        match self {
            SslModeKind::Off | SslModeKind::OffLoginOnly => Some(TransportWarning::Plaintext),
            SslModeKind::Prefer => Some(TransportWarning::MayBePlaintext),
            SslModeKind::RequiredUnverified => Some(TransportWarning::CertificateNotChecked),
            SslModeKind::VerifyCa => Some(TransportWarning::HostnameNotChecked),
            SslModeKind::VerifyFull => None,
        }
    }
}

/// A transport-security warning shown under the SSL mode selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportWarning {
    Plaintext,
    MayBePlaintext,
    CertificateNotChecked,
    HostnameNotChecked,
}

impl TransportWarning {
    pub fn message_key(self) -> &'static str {
        match self {
            TransportWarning::Plaintext => "connection_manager.ssl_warning.plaintext",
            TransportWarning::MayBePlaintext => "connection_manager.ssl_warning.may_be_plaintext",
            TransportWarning::CertificateNotChecked => {
                "connection_manager.ssl_warning.certificate_not_checked"
            }
            TransportWarning::HostnameNotChecked => {
                "connection_manager.ssl_warning.hostname_not_checked"
            }
        }
    }
}

/// A single SSL mode option that a driver declares in its metadata.
///
/// Built with [`SslModeOption::new`] and the flag methods, so a declaration
/// reads as what the mode is: `SslModeOption::new("require",
/// SslModeKind::RequiredUnverified).default_for_new()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SslModeOption {
    /// Driver-native identifier stored in the profile and passed to the driver
    /// (e.g. `"prefer"`, `"REQUIRED"`).
    pub id: &'static str,

    /// What the mode guarantees; also decides its label and warning.
    pub kind: SslModeKind,

    /// The mode a new connection starts with. Exactly one per driver, and it
    /// must always encrypt: list order never decides the default.
    pub default_for_new: bool,

    /// Kept so stored profiles keep working, but not offered for new
    /// selections: the selector only shows it while a profile uses it.
    pub legacy: bool,

    /// The mode the driver applies when a stored profile carries no mode at
    /// all (profiles saved before the mode was stored). Exactly one per
    /// driver; the form shows it when editing such a profile.
    pub when_unset: bool,
}

impl SslModeOption {
    pub const fn new(id: &'static str, kind: SslModeKind) -> Self {
        Self {
            id,
            kind,
            default_for_new: false,
            legacy: false,
            when_unset: false,
        }
    }

    pub const fn default_for_new(self) -> Self {
        Self {
            default_for_new: true,
            ..self
        }
    }

    pub const fn legacy(self) -> Self {
        Self {
            legacy: true,
            ..self
        }
    }

    pub const fn when_unset(self) -> Self {
        Self {
            when_unset: true,
            ..self
        }
    }
}

/// The mode a new connection of a driver with these modes starts with.
pub fn default_ssl_mode(modes: &'static [SslModeOption]) -> Option<&'static SslModeOption> {
    modes.iter().find(|mode| mode.default_for_new)
}

/// The mode the driver applies to a stored profile that carries none.
pub fn unset_ssl_mode(modes: &'static [SslModeOption]) -> Option<&'static SslModeOption> {
    modes.iter().find(|mode| mode.when_unset)
}

/// Looks up a mode by its id.
pub fn find_ssl_mode(modes: &'static [SslModeOption], id: &str) -> Option<&'static SslModeOption> {
    modes.iter().find(|mode| mode.id == id)
}

/// The modes the selector offers: every non-legacy mode, plus a legacy one
/// while it is the selected mode, so editing an old profile never silently
/// changes its mode.
pub fn visible_ssl_modes(
    modes: &'static [SslModeOption],
    selected: &str,
) -> Vec<&'static SslModeOption> {
    modes
        .iter()
        .filter(|mode| !mode.legacy || mode.id == selected)
        .collect()
}

/// The warning for `kind` on a connection whose configured database host is
/// `host`.
///
/// A loopback host gets none, also through an SSH tunnel: the database then
/// runs on the tunnel's server and the unencrypted hop never leaves that
/// machine. A blank host gets none until the user types one.
pub fn transport_warning(kind: SslModeKind, host: &str) -> Option<TransportWarning> {
    let host = host.trim();
    if host.is_empty() || is_loopback_host(host) {
        return None;
    }

    kind.warning()
}

/// Whether `host` names the local machine: a loopback IP address,
/// `localhost` or a `.localhost` name.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']);

    host.parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
        || host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
}

/// The error a verifying mode returns through an SSH tunnel when the
/// driver's client library cannot check the certificate against the
/// configured database host: the client only sees `127.0.0.1`, so a check
/// would test the wrong name, and skipping it would verify nothing.
pub fn tunnel_hostname_check_unsupported(driver: &str, db_host: &str, mode: &str) -> DbError {
    DbError::InvalidProfile(format!(
        "{driver} cannot check the server certificate against '{db_host}' through an SSH \
         tunnel: the client library only sees the tunnel's local address. Choose a mode \
         that does not check the hostname, or connect without the tunnel. (SSL mode '{mode}')"
    ))
}

/// The error for a stored SSL mode id the driver does not know. Unknown ids
/// fail instead of falling back to a weaker mode.
pub fn unknown_ssl_mode(driver: &str, mode: &str, modes: &[SslModeOption]) -> DbError {
    let known = modes
        .iter()
        .map(|mode| mode.id)
        .collect::<Vec<_>>()
        .join(", ");

    DbError::InvalidProfile(format!(
        "Unknown {driver} SSL mode '{mode}'. Expected one of: {known}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    static MODES: [SslModeOption; 4] = [
        SslModeOption::new("off", SslModeKind::Off),
        SslModeOption::new("prefer", SslModeKind::Prefer)
            .legacy()
            .when_unset(),
        SslModeOption::new("on", SslModeKind::RequiredUnverified).default_for_new(),
        SslModeOption::new("verify", SslModeKind::VerifyFull),
    ];

    #[test]
    fn the_default_comes_from_the_flag_not_the_list_order() {
        assert_eq!(default_ssl_mode(&MODES).map(|mode| mode.id), Some("on"));
        assert_eq!(unset_ssl_mode(&MODES).map(|mode| mode.id), Some("prefer"));
    }

    #[test]
    fn legacy_modes_are_only_offered_while_selected() {
        let ids = |selected| {
            visible_ssl_modes(&MODES, selected)
                .into_iter()
                .map(|mode| mode.id)
                .collect::<Vec<_>>()
        };

        assert_eq!(ids("on"), ["off", "on", "verify"]);
        assert_eq!(ids("prefer"), ["off", "prefer", "on", "verify"]);
    }

    #[test]
    fn warnings_follow_the_mode_and_skip_local_hosts() {
        assert_eq!(
            transport_warning(SslModeKind::Off, "db.example.com"),
            Some(TransportWarning::Plaintext)
        );
        assert_eq!(
            transport_warning(SslModeKind::Prefer, "10.0.0.5"),
            Some(TransportWarning::MayBePlaintext)
        );
        assert_eq!(
            transport_warning(SslModeKind::RequiredUnverified, "db.example.com"),
            Some(TransportWarning::CertificateNotChecked)
        );
        assert_eq!(
            transport_warning(SslModeKind::VerifyCa, "db.example.com"),
            Some(TransportWarning::HostnameNotChecked)
        );
        assert_eq!(
            transport_warning(SslModeKind::VerifyFull, "db.example.com"),
            None
        );

        for host in [
            "localhost",
            "127.0.0.1",
            "::1",
            "[::1]",
            "app.localhost",
            " ",
            "",
        ] {
            assert_eq!(transport_warning(SslModeKind::Off, host), None, "{host:?}");
        }
    }

    #[test]
    fn only_required_and_verifying_kinds_always_encrypt() {
        for kind in SslModeKind::ALL {
            let expected = matches!(
                kind,
                SslModeKind::RequiredUnverified | SslModeKind::VerifyCa | SslModeKind::VerifyFull
            );
            assert_eq!(kind.always_encrypts(), expected, "{kind:?}");
        }
    }
}
