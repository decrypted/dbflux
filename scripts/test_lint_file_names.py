"""Tests for the `file-names` check in lint.py; no cargo or Rust build needed."""
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

sys.path.insert(0, str(ROOT / "scripts"))
import lint  # noqa: E402


class SiteScanTests(unittest.TestCase):
    def scan(self, files):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative, text in files.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding="utf-8")
            return lint.file_name_sites(root)

    def test_flags_paths_built_from_names_outside_tests(self):
        sites = self.scan({
            "crates/dbflux_ui_x/src/export.rs": (
                "fn save(dir: &Path, table: &Table) {\n"
                "    let path = dir.join(&table.name);\n"
                "    dialog.set_file_name(&suggested_name);\n"
                "    dialog.set_file_name(suggested_name.as_str());\n"
                "    let fixed = dir.join(\"manifest.json\");\n"
                "}\n"
                "\n"
                "#[cfg(test)]\n"
                "mod tests {\n"
                "    fn t() { dir.join(name); }\n"
                "}\n"
            ),
            "crates/dbflux_ui_x/tests/it.rs": "fn t() { dir.join(name); }\n",
            "crates/dbflux_core/src/local_files/file_name.rs": "fn j() { dir.join(name); }\n",
        })

        self.assertEqual(
            sites,
            [
                ("crates/dbflux_ui_x/src/export.rs", "save", 2),
                ("crates/dbflux_ui_x/src/export.rs", "save", 3),
            ],
        )

    def test_ignores_joins_of_strings_and_unrelated_values(self):
        sites = self.scan({
            "crates/dbflux_x/src/lib.rs": (
                "fn f() {\n"
                "    let text = parts.join(\", \");\n"
                "    let path = dir.join(uuid);\n"
                "    let base = root.join(&relative);\n"
                "}\n"
            ),
        })

        self.assertEqual(sites, [])


class RepositoryTests(unittest.TestCase):
    def test_repository_allowlist_matches_the_code(self):
        allowed = lint.read_allowlist(lint.FILE_NAME_ALLOWLIST)

        self.assertIsNotNone(allowed)
        self.assertEqual(lint.allowlist_errors(lint.file_name_sites(), allowed, "site"), [])


if __name__ == "__main__":
    unittest.main()
