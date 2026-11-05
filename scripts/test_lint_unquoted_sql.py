"""Tests for the `unquoted-sql` check in lint.py; no cargo or Rust build needed."""
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

sys.path.insert(0, str(ROOT / "scripts"))
import lint  # noqa: E402


def literals(source):
    return [value for _, value in lint.rust_string_literals(source)]


class StringLiteralTests(unittest.TestCase):
    def test_decodes_escaped_quotes_and_skips_comments_and_chars(self):
        source = (
            '// "commented"\n'
            "/* \"block\" /* nested \"x\" */ */\n"
            "let q = '\"'; let s = \"USE \\\"{}\\\"\";\n"
        )

        self.assertEqual(literals(source), ['USE "{}"'])

    def test_reads_raw_strings_verbatim(self):
        self.assertEqual(literals('r#"SELECT "{}" FROM t"#'), ['SELECT "{}" FROM t'])

    def test_joins_continued_lines(self):
        self.assertEqual(literals('"SELECT \\\n     `{}`"'), ["SELECT `{}`"])


class SelfQuotedPlaceholderTests(unittest.TestCase):
    def test_flags_quoted_placeholders_in_queries(self):
        self.assertEqual(lint.self_quoted_placeholders("USE `{}`"), {"`"})
        self.assertEqual(lint.self_quoted_placeholders('SELECT * FROM "{name}"'), {'"'})
        self.assertEqual(lint.self_quoted_placeholders("USE [{}]"), {"["})
        self.assertEqual(lint.self_quoted_placeholders("WHERE c.relname = '{}'"), {"'"})

    def test_flags_bare_fragments(self):
        self.assertEqual(lint.self_quoted_placeholders("`{}`.`{}`"), {"`"})
        self.assertEqual(lint.self_quoted_placeholders('main."{}"'), {'"'})

    def test_ignores_helper_output_and_prose(self):
        self.assertEqual(lint.self_quoted_placeholders("USE {}"), set())
        self.assertEqual(lint.self_quoted_placeholders("SHOW INDEX FROM {}"), set())
        self.assertEqual(lint.self_quoted_placeholders("Table '{}' was not found"), set())
        self.assertEqual(lint.self_quoted_placeholders("duplicate key `{key}` in document"), set())

    def test_ignores_escaped_braces(self):
        self.assertEqual(lint.self_quoted_placeholders('SELECT "{{}}" FROM {}'), set())


class SiteScanTests(unittest.TestCase):
    def scan(self, files):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative, text in files.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding="utf-8")
            return lint.unquoted_sql_sites(root)

    def test_reports_format_macros_in_driver_code_only(self):
        sites = self.scan({
            "crates/dbflux_driver_x/src/driver.rs": (
                "fn switch(db: &str) {\n"
                "    conn.query_drop(format!(\"USE `{}`\", db));\n"
                "    let fixed = \"USE `{}`\";\n"
                "}\n"
                "\n"
                "#[cfg(test)]\n"
                "mod tests {\n"
                "    fn t() { format!(\"USE `{}`\", \"x\"); }\n"
                "}\n"
            ),
            "crates/dbflux_ui_x/src/view.rs": 'fn v() { format!("USE `{}`", db); }\n',
            "crates/dbflux_driver_x/tests/it.rs": 'fn t() { format!("USE `{}`", db); }\n',
        })

        self.assertEqual(sites, [("crates/dbflux_driver_x/src/driver.rs", "switch", 2)])

    def test_allowlist_counts_must_match_exactly(self):
        sites = [("a.rs", "f", 3), ("a.rs", "f", 9), ("b.rs", "g", 1)]

        errors = lint.allowlist_errors(sites, {("a.rs", "f"): 1, ("c.rs", "h"): 1}, "site")

        self.assertEqual(
            errors,
            [
                "a.rs:f: 1 unlisted site (lines 3, 9)",
                "b.rs:g: 1 unlisted site (lines 1)",
                "c.rs:h: 1 stale allowlist entry",
            ],
        )
        self.assertEqual(lint.allowlist_errors(sites, {("a.rs", "f"): 2, ("b.rs", "g"): 1}, "site"), [])


class RepositoryTests(unittest.TestCase):
    def test_repository_allowlist_matches_the_code(self):
        allowed = lint.read_allowlist(lint.UNQUOTED_SQL_ALLOWLIST)

        self.assertIsNotNone(allowed)
        self.assertEqual(lint.allowlist_errors(lint.unquoted_sql_sites(), allowed, "site"), [])


if __name__ == "__main__":
    unittest.main()
