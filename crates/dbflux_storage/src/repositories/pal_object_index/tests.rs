#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::bootstrap::StorageRuntime;

fn repo() -> PaletteObjectIndexRepo {
    let runtime = StorageRuntime::in_memory().expect("runtime");
    PaletteObjectIndexRepo::new(runtime.viz_connection().expect("connection"))
}

fn object(schema: Option<&str>, name: &str, kind: IndexedObjectKind) -> IndexedObject {
    IndexedObject {
        schema: schema.map(str::to_string),
        name: name.to_string(),
        kind,
    }
}

#[test]
fn a_replaced_database_reads_back_in_order() {
    let repo = repo();
    let objects = vec![
        object(Some("f3trade"), "f3_sport", IndexedObjectKind::Table),
        object(Some("f3trade"), "margin_rule", IndexedObjectKind::Table),
        object(Some("f3trade"), "sport_summary", IndexedObjectKind::View),
        object(None, "events", IndexedObjectKind::Collection),
    ];

    repo.replace_database("profile-1", "f3trade", &objects)
        .expect("replace");

    assert_eq!(
        repo.load_profile("profile-1").expect("load"),
        vec![IndexedDatabase {
            database: "f3trade".to_string(),
            objects,
        }]
    );
}

#[test]
fn replacing_a_database_drops_its_previous_objects_only() {
    let repo = repo();
    repo.replace_database(
        "profile-1",
        "f3trade",
        &[object(Some("f3trade"), "dropped", IndexedObjectKind::Table)],
    )
    .expect("first list");
    repo.replace_database(
        "profile-1",
        "f3base",
        &[object(
            Some("f3base"),
            "f3e_sport",
            IndexedObjectKind::Table,
        )],
    )
    .expect("other database");
    repo.replace_database(
        "profile-1",
        "f3trade",
        &[object(
            Some("f3trade"),
            "f3_sport",
            IndexedObjectKind::Table,
        )],
    )
    .expect("second list");

    let databases = repo.load_profile("profile-1").expect("load");

    assert_eq!(databases.len(), 2);
    assert_eq!(databases[0].database, "f3base");
    assert_eq!(databases[0].objects[0].name, "f3e_sport");
    assert_eq!(databases[1].database, "f3trade");
    assert_eq!(
        databases[1].objects,
        vec![object(
            Some("f3trade"),
            "f3_sport",
            IndexedObjectKind::Table
        )]
    );
}

#[test]
fn an_empty_list_removes_the_database() {
    let repo = repo();
    repo.replace_database(
        "profile-1",
        "f3trade",
        &[object(
            Some("f3trade"),
            "f3_sport",
            IndexedObjectKind::Table,
        )],
    )
    .expect("list");
    repo.replace_database("profile-1", "f3trade", &[])
        .expect("empty list");

    assert!(repo.load_profile("profile-1").expect("load").is_empty());
}

#[test]
fn profiles_do_not_see_each_other() {
    let repo = repo();
    repo.replace_database(
        "profile-1",
        "app",
        &[object(Some("public"), "users", IndexedObjectKind::Table)],
    )
    .expect("profile 1");
    repo.replace_database(
        "profile-2",
        "app",
        &[object(Some("public"), "orders", IndexedObjectKind::Table)],
    )
    .expect("profile 2");

    let first = repo.load_profile("profile-1").expect("load");

    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].objects,
        vec![object(Some("public"), "users", IndexedObjectKind::Table)]
    );
    assert!(repo.load_profile("profile-3").expect("load").is_empty());
}

#[test]
fn deleting_a_profile_forgets_only_its_databases() {
    let repo = repo();
    repo.replace_database(
        "profile-1",
        "app",
        &[object(Some("public"), "users", IndexedObjectKind::Table)],
    )
    .expect("profile 1");
    repo.replace_database(
        "profile-2",
        "app",
        &[object(Some("public"), "orders", IndexedObjectKind::Table)],
    )
    .expect("profile 2");

    repo.delete_profile("profile-1").expect("delete");

    assert!(repo.load_profile("profile-1").expect("load").is_empty());
    assert_eq!(repo.load_profile("profile-2").expect("load").len(), 1);
}

#[test]
fn retaining_profiles_forgets_the_others() {
    let repo = repo();
    for profile_id in ["kept", "deleted"] {
        repo.replace_database(
            profile_id,
            "app",
            &[object(Some("public"), "users", IndexedObjectKind::Table)],
        )
        .expect("list");
    }

    repo.retain_profiles(&["kept".to_string()]).expect("retain");

    assert_eq!(repo.load_profile("kept").expect("load").len(), 1);
    assert!(repo.load_profile("deleted").expect("load").is_empty());
}
