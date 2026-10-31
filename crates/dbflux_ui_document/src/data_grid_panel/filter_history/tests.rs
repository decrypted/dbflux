use super::{matching_entries, record_entry, search_words, word_ranges};

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn recording_moves_a_repeat_first_and_keeps_the_limit() {
    let mut entries = strings(&["b", "a", "c"]);

    record_entry(&mut entries, "a".to_string(), 3);
    assert_eq!(entries, strings(&["a", "b", "c"]), "no second copy of a");

    record_entry(&mut entries, "d".to_string(), 3);
    assert_eq!(entries, strings(&["d", "a", "b"]), "the oldest falls off");
}

#[test]
fn matching_is_a_case_insensitive_substring() {
    let entries = strings(&["status = 500", "Method = 'X'", "id > 10"]);

    assert_eq!(matching_entries(&entries, ""), vec![0, 1, 2]);
    assert_eq!(matching_entries(&entries, "  "), vec![0, 1, 2]);
    assert_eq!(matching_entries(&entries, "METHOD"), vec![1]);
    assert_eq!(matching_entries(&entries, "= "), vec![0, 1]);
    assert!(matching_entries(&entries, "nope").is_empty());
}

#[test]
fn every_word_must_match_in_any_order() {
    let entries = strings(&[
        "status = 500 AND method = 'x'",
        "status = 404",
        "method = 'getbets'",
    ]);

    assert_eq!(matching_entries(&entries, "status 500"), vec![0]);
    assert_eq!(matching_entries(&entries, "500   status"), vec![0]);
    assert_eq!(matching_entries(&entries, "METHOD"), vec![0, 2]);
    assert!(matching_entries(&entries, "status getbets").is_empty());
}

#[test]
fn word_ranges_are_sorted_merged_and_follow_the_original_text() {
    let words = search_words("STATUS 500 tus");
    assert_eq!(
        word_ranges("status = 500 AND Status = 5000", &words),
        vec![0..6, 9..12, 17..23, 26..29]
    );

    // A capital that lowercases to more bytes still maps onto its own
    // characters.
    let words = search_words("x");
    assert_eq!(word_ranges("\u{130}x", &words), vec![2..3]);

    assert!(word_ranges("anything", &[]).is_empty());
}
