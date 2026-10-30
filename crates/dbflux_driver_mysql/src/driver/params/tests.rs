use dbflux_core::{AccessKind, ConnectionProfile, DbConfig, DbKind, DriverParam};
use uuid::Uuid;

use super::super::{MysqlSslPaths, build_mysql_opts, build_mysql_uri_opts, extract_mysql_config};
use super::*;

fn params(pairs: &[(&str, &str)]) -> Vec<DriverParam> {
    pairs
        .iter()
        .map(|(key, value)| DriverParam::new(*key, *value))
        .collect()
}

fn mysql_profile(driver_params: Vec<DriverParam>) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new_with_kind(
        "aurora",
        DbKind::MySQL,
        DbConfig::MySQL {
            use_uri: false,
            uri: None,
            host: "aurora.cluster.example.com".to_string(),
            port: 3306,
            user: "app".to_string(),
            database: Some("app".to_string()),
            ssl_mode: Some("DISABLED".to_string()),
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

/// The options the driver builds for the first connection of `profile`,
/// through the same config extraction and parameter resolution as connect.
fn connect_opts(profile: &ConnectionProfile) -> Result<Opts, DbError> {
    let config = extract_mysql_config(&profile.config)?;
    let params = MysqlDriverParams::from_profile(profile)?;
    params.apply(build_mysql_opts(
        &config.host,
        config.port,
        &config.user,
        config.database.as_deref(),
        None,
        &config.ssl_mode,
        &MysqlSslPaths::default(),
    ))
}

fn error_text(result: Result<MysqlDriverParams, DbError>) -> String {
    result.expect_err("parameters must be refused").to_string()
}

#[test]
fn fields_mode_with_compress_true_enables_compression() {
    let profile = mysql_profile(params(&[("compress", "true")]));

    let opts = connect_opts(&profile).expect("parameters are valid");

    assert!(opts.get_compress().is_some());
    assert_eq!(opts.get_ip_or_hostname(), "aurora.cluster.example.com");
    assert_eq!(opts.get_user(), Some("app"));
}

#[test]
fn compression_survives_the_tunnel_redirect() {
    let mut profile = mysql_profile(params(&[("compress", "true"), ("stmt_cache_size", "7")]));
    profile.access_kind = Some(AccessKind::Proxy {
        proxy_profile_id: Uuid::new_v4(),
    });

    profile.config.redirect_to_tunnel(40123);
    let opts = connect_opts(&profile).expect("parameters are valid");

    assert_eq!(opts.get_ip_or_hostname(), "127.0.0.1");
    assert_eq!(opts.get_tcp_port(), 40123);
    assert!(opts.get_compress().is_some());
    assert_eq!(opts.get_stmt_cache_size(), 7);
}

#[test]
fn auto_compression_is_on_through_a_tunnel_and_off_direct() {
    let direct = mysql_profile(Vec::new());
    let mut tunneled = mysql_profile(Vec::new());
    tunneled.access_kind = Some(AccessKind::Ssh {
        ssh_tunnel_profile_id: Uuid::new_v4(),
    });
    tunneled.config.redirect_to_tunnel(40124);

    let direct_opts = connect_opts(&direct).expect("no parameters is valid");
    let tunneled_opts = connect_opts(&tunneled).expect("no parameters is valid");

    assert!(direct_opts.get_compress().is_none());
    assert!(tunneled_opts.get_compress().is_some());
    // Auto only asks for compression; everything else stays as the form set it.
    assert_eq!(tunneled_opts.get_user(), Some("app"));
    assert_eq!(tunneled_opts.get_db_name(), Some("app"));
}

#[test]
fn auto_compression_counts_the_inline_ssh_tunnel() {
    let mut profile = mysql_profile(Vec::new());
    if let DbConfig::MySQL {
        ssh_tunnel_profile_id,
        ..
    } = &mut profile.config
    {
        *ssh_tunnel_profile_id = Some(Uuid::new_v4());
    }

    let opts = connect_opts(&profile).expect("no parameters is valid");

    assert!(opts.get_compress().is_some());
}

#[test]
fn compress_false_turns_compression_off_even_through_a_tunnel() {
    let mut profile = mysql_profile(params(&[("compress", "false")]));
    profile.access_kind = Some(AccessKind::Ssh {
        ssh_tunnel_profile_id: Uuid::new_v4(),
    });

    let opts = connect_opts(&profile).expect("compress=false is valid");

    assert!(opts.get_compress().is_none());
}

#[test]
fn unknown_key_is_refused_with_the_key_named() {
    let message = error_text(MysqlDriverParams::resolve(
        &params(&[("compresss", "true")]),
        false,
    ));

    assert!(message.contains("`compresss`"), "{message}");
    assert!(
        message.contains("Unknown MySQL driver parameter"),
        "{message}"
    );
}

#[test]
fn invalid_value_is_refused_with_the_key_named() {
    let message = error_text(MysqlDriverParams::resolve(
        &params(&[("stmt_cache_size", "lots")]),
        false,
    ));

    assert!(message.contains("`stmt_cache_size`"), "{message}");
    assert!(message.contains("`lots`"), "{message}");
}

#[test]
fn form_owned_keys_are_refused_with_the_key_named() {
    for key in [
        "host",
        "port",
        "user",
        "password",
        "db_name",
        "socket",
        "prefer_socket",
        "ssl_mode",
    ] {
        let message = error_text(MysqlDriverParams::resolve(&params(&[(key, "x")]), false));
        assert!(message.contains(&format!("`{key}`")), "{key}: {message}");
        assert!(
            message.contains("set by the connection form"),
            "{key}: {message}"
        );
    }
}

#[test]
fn pool_keys_are_refused() {
    let message = error_text(MysqlDriverParams::resolve(
        &params(&[("pool_max", "4")]),
        false,
    ));

    assert!(message.contains("`pool_max`"), "{message}");
}

#[test]
fn explicit_parameters_win_over_uri_query_options() {
    let with_params = MysqlDriverParams::resolve(
        &params(&[("stmt_cache_size", "20"), ("compress", "false")]),
        false,
    )
    .expect("parameters are valid");
    let without_params = MysqlDriverParams::resolve(&[], false).expect("no parameters is valid");
    let uri = "mysql://app@db.example.com:3306/app?compress=true&stmt_cache_size=10";

    let overridden = build_mysql_uri_opts(uri, None, &with_params).expect("uri parses");
    let from_uri = build_mysql_uri_opts(uri, None, &without_params).expect("uri parses");

    assert_eq!(overridden.get_stmt_cache_size(), 20);
    assert!(overridden.get_compress().is_none());
    assert_eq!(from_uri.get_stmt_cache_size(), 10);
    assert!(from_uri.get_compress().is_some());
}

#[test]
fn validate_driver_params_uses_the_same_checks() {
    use dbflux_core::DbDriver;

    let driver = super::super::MysqlDriver::new(DbKind::MySQL);

    assert!(
        driver
            .validate_driver_params(&params(&[("compress", "best")]))
            .is_ok()
    );
    assert!(
        driver
            .validate_driver_params(&params(&[("host", "x")]))
            .is_err()
    );
    assert!(
        driver
            .driver_params_spec()
            .is_some_and(|spec| spec.compression.is_some())
    );
}
