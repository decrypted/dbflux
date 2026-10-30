use super::*;

const COMPRESSION: CompressionParam = CompressionParam {
    key: "compress",
    on_value: "true",
    off_value: "false",
};

fn params(pairs: &[(&str, &str)]) -> Vec<DriverParam> {
    pairs
        .iter()
        .map(|(key, value)| DriverParam::new(*key, *value))
        .collect()
}

#[test]
fn later_entries_win_and_blank_keys_are_dropped() {
    let list = params(&[("compress", "true"), (" ", "x"), ("compress", " best ")]);

    let map = driver_params_map(&list);

    assert_eq!(map.len(), 1);
    assert_eq!(map.get("compress").map(String::as_str), Some("best"));
    assert_eq!(driver_param_value(&list, "compress"), Some("best"));
}

#[test]
fn compression_choice_reads_the_parameter() {
    assert_eq!(COMPRESSION.choice(&[]), CompressionChoice::Auto);
    assert_eq!(
        COMPRESSION.choice(&params(&[("compress", "true")])),
        CompressionChoice::On
    );
    assert_eq!(
        COMPRESSION.choice(&params(&[("compress", "best")])),
        CompressionChoice::On
    );
    assert_eq!(
        COMPRESSION.choice(&params(&[("compress", "FALSE")])),
        CompressionChoice::Off
    );
}

#[test]
fn compression_apply_writes_and_removes_the_parameter() {
    let mut list = params(&[("stmt_cache_size", "64")]);

    COMPRESSION.apply(&mut list, CompressionChoice::On);
    assert_eq!(
        list,
        params(&[("stmt_cache_size", "64"), ("compress", "true")])
    );

    COMPRESSION.apply(&mut list, CompressionChoice::Off);
    assert_eq!(
        list,
        params(&[("stmt_cache_size", "64"), ("compress", "false")])
    );

    COMPRESSION.apply(&mut list, CompressionChoice::Auto);
    assert_eq!(list, params(&[("stmt_cache_size", "64")]));
}

#[test]
fn compression_apply_collapses_duplicates_in_place() {
    let mut list = params(&[
        ("compress", "fast"),
        ("tcp_keepalive_time_ms", "1"),
        ("compress", "false"),
    ]);

    COMPRESSION.apply(&mut list, CompressionChoice::On);

    assert_eq!(
        list,
        params(&[("compress", "true"), ("tcp_keepalive_time_ms", "1")])
    );
}

#[test]
fn compression_on_keeps_a_hand_written_level() {
    let mut list = params(&[("compress", "best")]);

    COMPRESSION.apply(&mut list, CompressionChoice::On);

    assert_eq!(list, params(&[("compress", "best")]));
}

#[test]
fn form_owned_keys_are_refused_with_the_key_named() {
    let map = driver_params_map(&params(&[("stmt_cache_size", "1"), ("Host", "evil")]));

    let error = refuse_form_owned_params("MySQL", &map, |key| key == "host")
        .expect_err("host is owned by the form");

    assert!(error.to_string().contains("`Host`"), "{error}");
}
