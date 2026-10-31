use super::{
    MAX_FILE_NAME_BYTES, SafeFileName, UniqueFileNames, confined_existing_file, confined_join,
    create_private_file, private_dir_in,
};

fn safe(name: &str) -> String {
    SafeFileName::new(name).as_str().to_string()
}

#[test]
fn ordinary_names_are_kept() {
    assert_eq!(safe("public.users.csv"), "public.users.csv");
    assert_eq!(safe("order_items-2024.json"), "order_items-2024.json");
    assert_eq!(safe("report final.csv"), "report final.csv");
}

#[test]
fn absolute_paths_become_one_component() {
    assert_eq!(safe("/etc/passwd"), "_etc_passwd");
    assert_eq!(
        safe("/tmp/./home/user/.config/Code/User/settings.json"),
        "_tmp_._home_user_.config_Code_User_settings.json"
    );
}

#[test]
fn parent_directory_steps_are_removed() {
    assert_eq!(safe("../../.bashrc"), "_._.bashrc");
    assert_eq!(safe(".."), "unnamed");
    assert_eq!(safe("."), "unnamed");
    assert_eq!(safe("a..b...c"), "a.b.c");
    assert!(!safe("x/../../y").contains(".."));
}

#[test]
fn backslashes_and_windows_punctuation_are_replaced() {
    assert_eq!(safe("..\\..\\Windows\\win.ini"), "_._Windows_win.ini");
    assert_eq!(safe("C:\\Users\\x"), "C__Users_x");
    assert_eq!(safe("a:b*c?d\"e<f>g|h"), "a_b_c_d_e_f_g_h");
}

#[test]
fn leading_dots_and_trailing_dots_and_spaces_are_trimmed() {
    assert_eq!(safe(".bashrc"), "bashrc");
    assert_eq!(safe("  ...hidden"), "hidden");
    assert_eq!(safe("name. . "), "name");
    assert_eq!(safe("trailing.csv.  "), "trailing.csv");
}

#[test]
fn control_and_invisible_characters_are_removed() {
    assert_eq!(safe("a\nb\tc\rd\0e\u{7f}f"), "a_b_c_d_e_f");
    // A right-to-left override would show `invoice_gpj.exe` as `invoice_exe.jpg`.
    assert_eq!(safe("invoice_\u{202E}gpj.exe"), "invoice_gpj.exe");
    assert_eq!(safe("zero\u{200B}width"), "zerowidth");
}

#[test]
fn windows_reserved_names_are_escaped() {
    assert_eq!(safe("CON"), "_CON");
    assert_eq!(safe("nul.csv"), "_nul.csv");
    assert_eq!(safe("Com1.users.json"), "_Com1.users.json");
    assert_eq!(safe("lpt9"), "_lpt9");
    assert_eq!(safe("COM¹"), "_COM¹");
    assert_eq!(safe("aux .txt"), "_aux .txt");
    assert_eq!(safe("console"), "console");
    assert_eq!(safe("COM10"), "COM10");
}

#[test]
fn empty_names_fall_back() {
    assert_eq!(safe(""), "unnamed");
    assert_eq!(safe("   "), "unnamed");
    assert_eq!(safe("\u{202E}"), "unnamed");
}

#[test]
fn unicode_letters_are_kept() {
    assert_eq!(safe("Ümlaut.Ölfeld.csv"), "Ümlaut.Ölfeld.csv");
    assert_eq!(safe("顧客.注文.json"), "顧客.注文.json");
    assert_eq!(safe("emoji🚀.csv"), "emoji🚀.csv");
}

#[test]
fn long_names_are_cut_at_a_character_boundary_keeping_the_extension() {
    let long = format!("{}.csv", "é".repeat(300));
    let name = safe(&long);

    assert!(name.len() <= MAX_FILE_NAME_BYTES, "{} bytes", name.len());
    assert!(name.ends_with(".csv"));
    assert!(name.starts_with('é'));
}

#[test]
fn the_sanitizer_is_idempotent() {
    for raw in [
        "/etc/passwd",
        "../x",
        "CON.txt",
        "a\\b",
        ".hidden",
        "plain.csv",
    ] {
        let once = safe(raw);
        assert_eq!(safe(&once), once, "{raw}");
    }
}

#[test]
fn numbered_names_go_before_the_extension() {
    let name = SafeFileName::new("users.csv");
    assert_eq!(name.numbered(2).as_str(), "users-2.csv");

    let no_extension = SafeFileName::new("README");
    assert_eq!(no_extension.numbered(3).as_str(), "README-3");

    let long = SafeFileName::new(&format!("{}.json", "x".repeat(400)));
    let numbered = long.numbered(12345);
    assert!(numbered.len() <= MAX_FILE_NAME_BYTES);
    assert!(numbered.ends_with("-12345.json"));
}

#[test]
fn colliding_names_get_a_number_without_regard_to_case() {
    let mut names = UniqueFileNames::new();

    assert_eq!(
        names.claim(SafeFileName::new("a/b.csv")).as_str(),
        "a_b.csv"
    );
    assert_eq!(
        names.claim(SafeFileName::new("a_b.csv")).as_str(),
        "a_b-2.csv"
    );
    assert_eq!(
        names.claim(SafeFileName::new("A_B.csv")).as_str(),
        "A_B-3.csv"
    );
    assert_eq!(names.claim(SafeFileName::new("c.csv")).as_str(), "c.csv");
}

#[test]
fn confined_join_stays_in_the_directory() {
    let directory = tempfile::tempdir().expect("temp dir");

    let path = confined_join(directory.path(), &SafeFileName::new("/etc/passwd"))
        .expect("a sanitized name joins");

    assert_eq!(path.parent(), Some(directory.path()));
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("_etc_passwd")
    );
}

#[test]
fn bundle_file_names_must_be_plain_files_inside_the_bundle() {
    let bundle = tempfile::tempdir().expect("temp dir");
    let outside = tempfile::tempdir().expect("temp dir");
    std::fs::write(bundle.path().join("public.users.csv"), "id\n1\n").expect("write");
    std::fs::write(outside.path().join("secret"), "token").expect("write");

    assert!(confined_existing_file(bundle.path(), "public.users.csv").is_ok());

    let absolute = outside.path().join("secret");
    let absolute = absolute.to_str().expect("utf-8 temp path");
    for name in [
        absolute,
        "../secret",
        "sub/file.csv",
        "..\\secret",
        "..",
        "",
    ] {
        assert!(
            confined_existing_file(bundle.path(), name).is_err(),
            "{name:?} must be refused"
        );
    }
}

#[cfg(unix)]
#[test]
fn bundle_file_names_refuse_symbolic_links() {
    let bundle = tempfile::tempdir().expect("temp dir");
    let outside = tempfile::tempdir().expect("temp dir");
    let secret = outside.path().join("credentials");
    std::fs::write(&secret, "aws_secret_access_key = x").expect("write");
    std::os::unix::fs::symlink(&secret, bundle.path().join("public.users.csv")).expect("symlink");

    let error = confined_existing_file(bundle.path(), "public.users.csv")
        .expect_err("a link out of the bundle must be refused");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use super::super::private_dir::effective_uid;
    use super::{SafeFileName, create_private_file, private_dir_in};

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::symlink_metadata(path).expect("metadata").mode() & 0o777
    }

    #[test]
    fn a_new_private_dir_is_owner_only_and_ours() {
        let parent = tempfile::tempdir().expect("temp dir");

        let dir = private_dir_in(parent.path(), "dbflux-open").expect("private dir");

        assert_eq!(mode(&dir), 0o700);
        assert_eq!(
            std::fs::metadata(&dir).expect("metadata").uid(),
            effective_uid()
        );
        assert_eq!(dir, parent.path().join("dbflux-open"));
    }

    #[test]
    fn an_existing_readable_dir_of_ours_is_narrowed() {
        let parent = tempfile::tempdir().expect("temp dir");
        let existing = parent.path().join("dbflux-open");
        std::fs::create_dir(&existing).expect("create");
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        private_dir_in(parent.path(), "dbflux-open").expect("private dir");

        assert_eq!(mode(&existing), 0o700);
    }

    #[test]
    fn a_dir_others_can_write_to_is_refused() {
        let parent = tempfile::tempdir().expect("temp dir");
        let existing = parent.path().join("dbflux-open");
        std::fs::create_dir(&existing).expect("create");
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o777)).expect("chmod");

        let error = private_dir_in(parent.path(), "dbflux-open")
            .expect_err("a world-writable dir must be refused");

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_symbolic_link_in_place_of_the_dir_is_refused() {
        let parent = tempfile::tempdir().expect("temp dir");
        let elsewhere = tempfile::tempdir().expect("temp dir");
        std::os::unix::fs::symlink(elsewhere.path(), parent.path().join("dbflux-open"))
            .expect("symlink");

        let error = private_dir_in(parent.path(), "dbflux-open")
            .expect_err("a link must not be used as the private dir");

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn private_files_are_new_owner_only_and_never_follow_links() {
        let dir = tempfile::tempdir().expect("temp dir");
        let elsewhere = tempfile::tempdir().expect("temp dir");
        let target = elsewhere.path().join("planted");
        let name = SafeFileName::new("report.pdf");
        std::os::unix::fs::symlink(&target, dir.path().join("report.pdf")).expect("symlink");

        let path = create_private_file(dir.path(), &name).expect("private file");

        assert_eq!(path, dir.path().join("report-2.pdf"));
        assert_eq!(mode(&path), 0o600);
        assert!(
            !target.exists(),
            "the dangling link must not have been followed"
        );
    }

    #[test]
    fn private_files_never_reuse_an_existing_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("data.csv"), "keep").expect("write");
        let name = SafeFileName::new("data.csv");

        let first = create_private_file(dir.path(), &name).expect("private file");
        let second = create_private_file(dir.path(), &name).expect("private file");

        assert_eq!(first, dir.path().join("data-2.csv"));
        assert_eq!(second, dir.path().join("data-3.csv"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("data.csv")).expect("read"),
            "keep"
        );
    }
}

#[test]
fn private_dir_names_are_sanitized() {
    let parent = tempfile::tempdir().expect("temp dir");

    let dir = private_dir_in(parent.path(), "../escape").expect("private dir");

    assert_eq!(dir.parent(), Some(parent.path()));
}

#[test]
fn private_files_take_sanitized_names_only() {
    let dir = tempfile::tempdir().expect("temp dir");

    let path =
        create_private_file(dir.path(), &SafeFileName::new("../../evil.sh")).expect("private file");

    assert_eq!(path.parent(), Some(dir.path()));
}
