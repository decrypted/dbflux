use super::{matching_entries, record_entry};

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
