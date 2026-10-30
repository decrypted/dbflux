use std::time::Duration;

use dbflux_core::{AccessKind, ConnectionProfile, DbConfig, DriverParam};
use postgres::config::TargetSessionAttrs;
use uuid::Uuid;

use super::super::{
    PostgresConnectParams, direct_conn_string, extract_postgres_config, parse_pg_sslmode,
    uri_conn_string,
};
use super::*;
use crate::tls::PgTlsFiles;

fn params(pairs: &[(&str, &str)]) -> Vec<DriverParam> {
    pairs
        .iter()
        .map(|(key, value)| DriverParam::new(*key, *value))
        .collect()
}

fn postgres_profile(driver_params: Vec<DriverParam>) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new(
        "warehouse",
        DbConfig::Postgres {
            use_uri: false,
            uri: None,
            host: "db.example.com".to_string(),
            port: 5432,
            user: "app".to_string(),
            database: "app".to_string(),
            ssl_mode: Some("disable".to_string()),
            ssl_root_cert_path: None,
            ssl_client_cert_path: None,
            ssl_client_key_path: None,
            ssh_tunnel: None,
            ssh_tunnel_profile_id: None,
        },
    );
    profile.driver_params = driver_params;
    profile
}

/// The client config the driver builds for `profile` in fields mode.
fn fields_config(profile: &ConnectionProfile) -> postgres::Config {
    let config = extract_postgres_config(&profile.config).expect("postgres config");
    let driver_params = PgDriverParams::from_profile(profile).expect("parameters are valid");
    let tls_files = PgTlsFiles::default();
    let connect = PostgresConnectParams {
        host: &config.host,
        port: config.port,
        user: &config.user,
        password: None,
        database: &config.database,
        ssl_mode: &config.ssl_mode,
        tls_files: &tls_files,
        driver_params: &driver_params,
    };

    direct_conn_string(&connect, parse_pg_sslmode(&config.ssl_mode))
        .parse()
        .expect("generated connection string parses")
}

fn error_text(result: Result<PgDriverParams, DbError>) -> String {
    result.expect_err("parameters must be refused").to_string()
}

#[test]
fn fields_mode_maps_parameters_onto_the_client_config() {
    let profile = postgres_profile(params(&[
        ("connect_timeout", "5"),
        ("keepalives_idle", "45"),
        ("options", "-c statement_timeout=5000"),
        ("target_session_attrs", "read-write"),
    ]));

    let config = fields_config(&profile);

    assert_eq!(config.get_connect_timeout(), Some(&Duration::from_secs(5)));
    assert_eq!(config.get_keepalives_idle(), Duration::from_secs(45));
    assert_eq!(config.get_options(), Some("-c statement_timeout=5000"));
    assert_eq!(
        config.get_target_session_attrs(),
        TargetSessionAttrs::ReadWrite
    );
    assert_eq!(config.get_user(), Some("app"));
    assert_eq!(config.get_dbname(), Some("app"));
}

#[test]
fn without_parameters_the_dbflux_defaults_stay() {
    let config = fields_config(&postgres_profile(Vec::new()));

    assert_eq!(config.get_connect_timeout(), Some(&Duration::from_secs(30)));
    assert_eq!(
        config.get_application_name(),
        Some(dbflux_core::client_identity())
    );
}

#[test]
fn an_explicit_application_name_replaces_the_client_identity() {
    let config = fields_config(&postgres_profile(params(&[(
        "application_name",
        "nightly report",
    )])));

    assert_eq!(config.get_application_name(), Some("nightly report"));
}

#[test]
fn parameters_survive_the_tunnel_redirect() {
    let mut profile = postgres_profile(params(&[("connect_timeout", "7")]));
    profile.access_kind = Some(AccessKind::Ssh {
        ssh_tunnel_profile_id: Uuid::new_v4(),
    });

    profile.config.redirect_to_tunnel(40125);
    let config = fields_config(&profile);

    assert_eq!(config.get_ports(), &[40125]);
    assert_eq!(config.get_connect_timeout(), Some(&Duration::from_secs(7)));
}

#[test]
fn explicit_parameters_win_over_uri_query_options() {
    let driver_params =
        PgDriverParams::resolve(&params(&[("connect_timeout", "9"), ("options", "-c a=b")]))
            .expect("parameters are valid");
    let uri = "postgresql://app@db.example.com/app?connect_timeout=3&application_name=mine";

    let overridden: postgres::Config = uri_conn_string(uri, None, &driver_params)
        .parse()
        .expect("uri parses");
    let from_uri: postgres::Config = uri_conn_string(uri, None, &PgDriverParams::default())
        .parse()
        .expect("uri parses");

    assert_eq!(
        overridden.get_connect_timeout(),
        Some(&Duration::from_secs(9))
    );
    assert_eq!(overridden.get_options(), Some("-c a=b"));
    assert_eq!(overridden.get_application_name(), Some("mine"));
    assert_eq!(
        from_uri.get_connect_timeout(),
        Some(&Duration::from_secs(3))
    );
}

#[test]
fn unknown_key_is_refused_with_the_key_named() {
    let message = error_text(PgDriverParams::resolve(&params(&[(
        "statement_timeout",
        "1",
    )])));

    assert!(message.contains("`statement_timeout`"), "{message}");
    assert!(
        message.contains("Unknown PostgreSQL driver parameter"),
        "{message}"
    );
}

#[test]
fn invalid_value_is_refused_with_the_key_named() {
    let message = error_text(PgDriverParams::resolve(&params(&[(
        "target_session_attrs",
        "primary",
    )])));

    assert!(message.contains("`target_session_attrs`"), "{message}");
    assert!(message.contains("`primary`"), "{message}");
}

#[test]
fn form_owned_keys_are_refused_with_the_key_named() {
    for key in [
        "host",
        "port",
        "user",
        "password",
        "dbname",
        "sslmode",
        "sslrootcert",
    ] {
        let message = error_text(PgDriverParams::resolve(&params(&[(key, "x")])));
        assert!(message.contains(&format!("`{key}`")), "{key}: {message}");
        assert!(
            message.contains("set by the connection form"),
            "{key}: {message}"
        );
    }
}

#[test]
fn driver_declares_parameters_without_compression() {
    use dbflux_core::DbDriver;

    let driver = super::super::PostgresDriver::new();

    assert!(
        driver
            .driver_params_spec()
            .is_some_and(|spec| spec.compression.is_none())
    );
    assert!(
        driver
            .validate_driver_params(&params(&[("keepalives", "0")]))
            .is_ok()
    );
    assert!(
        driver
            .validate_driver_params(&params(&[("sslmode", "disable")]))
            .is_err()
    );
}
