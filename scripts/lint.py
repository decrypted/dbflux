#!/usr/bin/env python3
"""Run rustfmt, clippy, or the static source checks on the first-party crates of DBFlux.

First-party crates are the workspace packages whose manifest lives under
`<repo>/crates/`. The crates under `vendor/` are third-party code kept in the
workspace, so this script never lints or reformats them. CI and local
development both call this script, which keeps that selection in one place.

Usage:
    scripts/lint.py fmt [--check]
    scripts/lint.py clippy [extra cargo arguments...]
    scripts/lint.py mouse-down
    scripts/lint.py unquoted-sql
    scripts/lint.py file-names

Arguments given to `clippy` are passed to cargo verbatim before `--`, for
example `--locked --features "sqlite,postgres"` or `--tests`.

`mouse-down` fails on every left-button `on_mouse_down` handler in non-test code
under `crates/` that is not listed in `scripts/mouse_down_allowlist.txt`.
Activations belong in `on_click`, which runs on release and is exposed to
assistive technology as a click action; `on_mouse_down` is reserved for press
semantics such as drag starts, resize grips and focus guards.

`unquoted-sql` fails on every `format!`-style string in non-test code of the
driver crates and `dbflux_core` that wraps a placeholder in its own quotes,
such as "`{}`", "\"{}\"", "[{}]" or, inside a query, "'{}'", unless the site
is listed in `scripts/unquoted_sql_allowlist.txt`. Names and values in SQL go
through the dialect's quoting helper; a hand-written pair of quotes around a
placeholder is how a server-provided name that contains the quote character
ends up running as SQL.

`file-names` fails on every `Path::join`, `with_file_name` or `set_file_name`
call in non-test code under `crates/` whose argument is a variable or field
called like a name (`name`, `table`, `schema`, `key`, `file`, `title`,
`object`), unless the site is listed in `scripts/file_name_allowlist.txt`. A
table, collection or object name comes from the server, and `join` with an
absolute or `..` name writes outside the directory; such names become
`dbflux_core::local_files::SafeFileName` and are placed with `confined_join`.
"""

import argparse
import json
import re
import shlex
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MOUSE_DOWN_ALLOWLIST = ROOT / "scripts" / "mouse_down_allowlist.txt"
UNQUOTED_SQL_ALLOWLIST = ROOT / "scripts" / "unquoted_sql_allowlist.txt"
FILE_NAME_ALLOWLIST = ROOT / "scripts" / "file_name_allowlist.txt"
FILE_NAME_EXEMPT = ("crates/dbflux_core/src/local_files/",)
NAME_TO_PATH = re.compile(
    r"\.(?:join|with_file_name|set_file_name)\(\s*&?(?:[A-Za-z_][\w.]*\.)?\w*"
    r"(?:name|table|schema|key|file|title|object)\w*(?:\(\))?\s*\)"
)
UNQUOTED_SQL_SOURCES = ("crates/dbflux_driver_*/src/**/*.rs", "crates/dbflux_core/src/**/*.rs")
FORMAT_MACRO_CALL = re.compile(r"\b(?:format|write|writeln|format_args)!\s*\(\s*(?:[^,()\"]+,\s*)?$")
SQL_KEYWORD = re.compile(
    r"\b(?:SELECT|FROM|WHERE|USE|SHOW|INSERT|UPDATE|DELETE|ALTER|CREATE|DROP|TRUNCATE|PRAGMA|KILL"
    r"|DESCRIBE|EXEC|EXECUTE|GRANT|SET|ATTACH|DETACH|VALUES|INTO|JOIN|LIKE|AND|OR|TABLE|INDEX|TYPE"
    r"|VALUE|IN|DATABASE|SCHEMA|OPTIMIZE|VACUUM|ANALYZE|EXPLAIN|RENAME|COMMENT|CALL|LIMIT|ORDER"
    r"|GROUP|BY|RETURNING|CAST|DEFAULT|CONSTRAINT|REFERENCES|COLUMN|VIEW|TRIGGER|PROCEDURE"
    r"|FUNCTION)\b"
)
FLUX_MARKERS = ("|>", "from(bucket")
IDENTIFIER_QUOTES = {"`": "`", '"': '"', "[": "]"}
RUST_CHAR_LITERAL = re.compile(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'\n])'")
RUST_RAW_STRING_START = re.compile(r'b?r(#*)"')
RUST_STRING_ESCAPES = {'"': '"', "\\": "\\", "n": "\n", "t": "\t", "r": "\r", "'": "'", "0": "\0"}
LEFT_MOUSE_DOWN = re.compile(r"\.on_mouse_down\(\s*(?:gpui::)?MouseButton::Left\b")
FUNCTION_HEADER = re.compile(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)")
TEST_MODULE_HEADER = re.compile(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")


def first_party_packages() -> list[str]:
    """Return the sorted names of workspace packages whose manifest is under `crates/`.

    Exits with an error when none are found, so a broken metadata query can
    never turn into a lint run that silently checks nothing.
    """
    metadata = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )

    crates_dir = (ROOT / "crates").resolve()
    packages = sorted(
        package["name"]
        for package in json.loads(metadata.stdout)["packages"]
        if Path(package["manifest_path"]).resolve().is_relative_to(crates_dir)
    )

    if not packages:
        sys.exit("No first-party crates found under crates/; refusing to lint nothing")

    return packages


def package_arguments(packages: list[str]) -> list[str]:
    """Expand package names into repeated `--package <name>` cargo arguments."""
    return [argument for name in packages for argument in ("--package", name)]


def run(command: list[str]) -> int:
    """Print the command to stderr, run it from the repository root, and return its exit code."""
    print(shlex.join(command), file=sys.stderr, flush=True)

    return subprocess.run(command, cwd=ROOT).returncode


def is_test_path(path: Path, root: Path = ROOT) -> bool:
    """Whether a source file only holds tests: a `tests/` directory or a `tests.rs` module."""
    relative = path.relative_to(root)

    return "tests" in relative.parts[:-1] or path.stem == "tests" or path.stem.endswith("_tests")


def test_module_lines(lines: list[str]) -> set[int]:
    """Return the zero-based line numbers that sit inside `#[cfg(test)] mod name { ... }` blocks.

    rustfmt closes a module with a `}` at the indentation of its `mod` line, so
    the block ends at the first such line after the header.
    """
    inside: set[int] = set()
    index = 0

    while index < len(lines):
        if lines[index].strip() != "#[cfg(test)]":
            index += 1
            continue

        header_index = index + 1
        while header_index < len(lines) and lines[header_index].strip().startswith("#["):
            header_index += 1

        header = TEST_MODULE_HEADER.match(lines[header_index]) if header_index < len(lines) else None
        if header is None:
            index += 1
            continue

        closing = header.group(1) + "}"
        end_index = header_index + 1
        while end_index < len(lines) and lines[end_index].rstrip() != closing:
            end_index += 1

        inside.update(range(index, end_index + 1))
        index = end_index + 1

    return inside


def enclosing_function(lines: list[str], line_index: int) -> str:
    """Name the innermost `fn` whose header is less indented than the handler line."""
    site_indent = len(lines[line_index]) - len(lines[line_index].lstrip())

    for index in range(line_index, -1, -1):
        header = FUNCTION_HEADER.match(lines[index])
        if header and len(header.group(1)) < site_indent:
            return header.group(2)

    return "<module>"


def left_mouse_down_sites() -> list[tuple[str, str, int]]:
    """Find every non-test left-button `on_mouse_down` in `crates/` as (path, function, line)."""
    sites = []

    for path in sorted((ROOT / "crates").rglob("*.rs")):
        if is_test_path(path):
            continue

        text = path.read_text(encoding="utf-8")
        if "on_mouse_down" not in text:
            continue

        lines = text.split("\n")
        excluded = test_module_lines(lines)
        relative = path.relative_to(ROOT).as_posix()

        for match in LEFT_MOUSE_DOWN.finditer(text):
            line_index = text.count("\n", 0, match.start())
            if line_index in excluded:
                continue

            sites.append((relative, enclosing_function(lines, line_index), line_index + 1))

    return sites


def read_allowlist(allowlist: Path) -> dict[tuple[str, str], int] | None:
    """Parse `path:function: reason` entries into a count per (path, function).

    One entry covers one site, so a function with three allowed sites is
    listed three times. Returns None after printing the offending lines when
    an entry is malformed.
    """
    allowed: dict[tuple[str, str], int] = {}
    malformed = []

    if not allowlist.exists():
        return allowed

    for number, raw in enumerate(allowlist.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue

        parts = [part.strip() for part in line.split(":", 2)]
        if len(parts) != 3 or not all(parts):
            malformed.append(f"{allowlist.name}:{number}: expected `path:function: reason`")
            continue

        key = (parts[0], parts[1])
        allowed[key] = allowed.get(key, 0) + 1

    if malformed:
        print("\n".join(malformed), file=sys.stderr)
        return None

    return allowed


def allowlist_errors(
    sites: list[tuple[str, str, int]], allowed: dict[tuple[str, str], int], unlisted: str
) -> list[str]:
    """Compare found (path, function, line) sites with allowlist counts.

    Reports every function with more sites than entries, using `unlisted` to
    describe them, and every entry left over once its sites are gone, so the
    allowlist always matches the code exactly.
    """
    found: dict[tuple[str, str], list[int]] = {}
    for path, function, line in sites:
        found.setdefault((path, function), []).append(line)

    errors = []
    for key, lines in sorted(found.items()):
        missing = len(lines) - allowed.get(key, 0)
        if missing > 0:
            locations = ", ".join(str(line) for line in lines)
            errors.append(f"{key[0]}:{key[1]}: {missing} unlisted {unlisted} (lines {locations})")

    for key, count in sorted(allowed.items()):
        stale = count - len(found.get(key, []))
        if stale > 0:
            errors.append(f"{key[0]}:{key[1]}: {stale} stale allowlist entr{'y' if stale == 1 else 'ies'}")

    return errors


def check_mouse_down() -> int:
    """Fail on left-button `on_mouse_down` handlers missing from the allowlist, and on stale entries."""
    allowed = read_allowlist(MOUSE_DOWN_ALLOWLIST)
    if allowed is None:
        return 1

    sites = left_mouse_down_sites()
    errors = allowlist_errors(
        sites,
        allowed,
        "left-button on_mouse_down; use on_click for activations, or allowlist a press handler",
    )

    if errors:
        print("\n".join(errors), file=sys.stderr)
        print(f"mouse-down: {len(errors)} problem(s), {len(sites)} handler(s) found", file=sys.stderr)
        return 1

    print(f"mouse-down: {len(sites)} allowlisted press handler(s), no unlisted activations")
    return 0


def rust_string_literals(text: str):
    """Yield (offset, value) for every string literal outside comments.

    Escapes in ordinary strings are decoded, so `\\"` comes back as `"`; raw
    strings come back verbatim. Char literals and lifetimes are skipped so a
    `'"'` never opens a string.
    """
    index, length = 0, len(text)

    while index < length:
        if text.startswith("//", index):
            end = text.find("\n", index)
            index = length if end < 0 else end
            continue

        if text.startswith("/*", index):
            depth, index = 1, index + 2
            while index < length and depth:
                if text.startswith("/*", index):
                    depth, index = depth + 1, index + 2
                elif text.startswith("*/", index):
                    depth, index = depth - 1, index + 2
                else:
                    index += 1
            continue

        character = text[index]
        follows_identifier = index > 0 and (text[index - 1].isalnum() or text[index - 1] == "_")

        raw = RUST_RAW_STRING_START.match(text, index) if character in "br" and not follows_identifier else None
        if raw:
            closing = '"' + raw.group(1)
            end = text.find(closing, raw.end())
            end = length if end < 0 else end
            yield index, text[raw.end():end]
            index = end + len(closing)
            continue

        if character == '"':
            cursor, value = index + 1, []
            while cursor < length and text[cursor] != '"':
                if text[cursor] == "\\" and cursor + 1 < length:
                    escaped = text[cursor + 1]
                    if escaped == "\n":
                        cursor += 2
                        while cursor < length and text[cursor] in " \t\n":
                            cursor += 1
                        continue
                    value.append(RUST_STRING_ESCAPES.get(escaped, "\\" + escaped))
                    cursor += 2
                    continue
                value.append(text[cursor])
                cursor += 1
            yield index, "".join(value)
            index = cursor + 1
            continue

        if character == "'":
            char_literal = RUST_CHAR_LITERAL.match(text, index)
            if char_literal:
                index = char_literal.end()
                continue

        index += 1


def format_placeholders(value: str):
    """Yield (start, end) of each `{...}` placeholder, skipping `{{` and `}}`."""
    index = 0

    while index < len(value):
        if value.startswith("{{", index) or value.startswith("}}", index):
            index += 2
        elif value[index] == "{":
            end = value.find("}", index)
            if end < 0:
                return
            yield index, end + 1
            index = end + 1
        else:
            index += 1


def self_quoted_placeholders(value: str) -> set[str]:
    """Return the opening quotes this format string puts directly around a placeholder.

    Identifier quotes count in query text and in bare fragments without
    whitespace (the way helpers and partial statements are built); a single
    quote only counts in query text, since prose messages quote values that
    way all the time.
    """
    literal_text = re.sub(r"\{[^{}]*\}", "", value.replace("{{", "").replace("}}", ""))
    is_query = bool(SQL_KEYWORD.search(literal_text)) or any(marker in value for marker in FLUX_MARKERS)
    is_fragment = not re.search(r"\s", literal_text)
    if not (is_query or is_fragment):
        return set()

    quotes = dict(IDENTIFIER_QUOTES)
    if is_query:
        quotes["'"] = "'"

    found = set()
    for start, end in format_placeholders(value):
        before = value[start - 1] if start > 0 else ""
        after = value[end] if end < len(value) else ""
        if before in quotes and quotes[before] == after:
            found.add(before)

    return found


def unquoted_sql_sites(root: Path = ROOT) -> list[tuple[str, str, int]]:
    """Find every self-quoted placeholder in non-test format strings as (path, function, line)."""
    sites = []
    paths = sorted({path for pattern in UNQUOTED_SQL_SOURCES for path in root.glob(pattern)})

    for path in paths:
        if is_test_path(path, root):
            continue

        text = path.read_text(encoding="utf-8")
        lines = text.split("\n")
        excluded = test_module_lines(lines)
        relative = path.relative_to(root).as_posix()

        for offset, value in rust_string_literals(text):
            line_index = text.count("\n", 0, offset)
            if line_index in excluded:
                continue
            if not FORMAT_MACRO_CALL.search(text, max(0, offset - 200), offset):
                continue
            if self_quoted_placeholders(value):
                sites.append((relative, enclosing_function(lines, line_index), line_index + 1))

    return sites


def check_unquoted_sql() -> int:
    """Fail on self-quoted SQL placeholders missing from the allowlist, and on stale entries."""
    allowed = read_allowlist(UNQUOTED_SQL_ALLOWLIST)
    if allowed is None:
        return 1

    sites = unquoted_sql_sites()
    errors = allowlist_errors(
        sites,
        allowed,
        "self-quoted placeholder; quote names and values with the dialect's helper, "
        "or allowlist a site that is provably safe",
    )

    if errors:
        print("\n".join(errors), file=sys.stderr)
        print(f"unquoted-sql: {len(errors)} problem(s), {len(sites)} site(s) found", file=sys.stderr)
        return 1

    print(f"unquoted-sql: {len(sites)} allowlisted site(s), no unlisted self-quoted placeholders")
    return 0


def file_name_sites(root: Path = ROOT) -> list[tuple[str, str, int]]:
    """Find every non-test path built from a name-like value as (path, function, line)."""
    sites = []

    for path in sorted((root / "crates").rglob("*.rs")):
        if is_test_path(path, root):
            continue

        relative = path.relative_to(root).as_posix()
        if relative.startswith(FILE_NAME_EXEMPT):
            continue

        text = path.read_text(encoding="utf-8")
        lines = text.split("\n")
        excluded = test_module_lines(lines)

        for match in NAME_TO_PATH.finditer(text):
            line_index = text.count("\n", 0, match.start())
            if line_index in excluded:
                continue

            sites.append((relative, enclosing_function(lines, line_index), line_index + 1))

    return sites


def check_file_names() -> int:
    """Fail on paths built from unlisted name-like values, and on stale allowlist entries."""
    allowed = read_allowlist(FILE_NAME_ALLOWLIST)
    if allowed is None:
        return 1

    sites = file_name_sites()
    errors = allowlist_errors(
        sites,
        allowed,
        "path built from a name; make a server-provided name a SafeFileName and place it "
        "with confined_join, or allowlist a name that never comes from a server",
    )

    if errors:
        print("\n".join(errors), file=sys.stderr)
        print(f"file-names: {len(errors)} problem(s), {len(sites)} site(s) found", file=sys.stderr)
        return 1

    print(f"file-names: {len(sites)} allowlisted site(s), no unlisted paths built from names")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run rustfmt, clippy, or static checks on first-party crates only (never vendor/)."
    )
    subcommands = parser.add_subparsers(dest="command", required=True)

    fmt_parser = subcommands.add_parser("fmt", help="Format first-party crates with rustfmt")
    fmt_parser.add_argument(
        "--check", action="store_true", help="Report formatting differences without writing"
    )

    subcommands.add_parser(
        "clippy", help="Lint first-party crates with clippy; extra arguments go to cargo"
    )

    subcommands.add_parser(
        "mouse-down",
        help="Fail on left-button on_mouse_down handlers missing from scripts/mouse_down_allowlist.txt",
    )

    subcommands.add_parser(
        "unquoted-sql",
        help="Fail on self-quoted SQL placeholders missing from scripts/unquoted_sql_allowlist.txt",
    )

    subcommands.add_parser(
        "file-names",
        help="Fail on paths built from names missing from scripts/file_name_allowlist.txt",
    )

    arguments, extra = parser.parse_known_args()

    if arguments.command in ("fmt", "mouse-down", "unquoted-sql", "file-names") and extra:
        parser.error(f"unrecognized arguments: {' '.join(extra)}")

    if arguments.command == "mouse-down":
        return check_mouse_down()

    if arguments.command == "unquoted-sql":
        return check_unquoted_sql()

    if arguments.command == "file-names":
        return check_file_names()

    packages = package_arguments(first_party_packages())

    if arguments.command == "fmt":
        command = ["cargo", "fmt", *packages]
        if arguments.check:
            command += ["--", "--check"]

        return run(command)

    return run([
        "cargo",
        "clippy",
        "--no-deps",
        # Lint test, bench and example targets too, not only lib/bin defaults.
        "--all-targets",
        *packages,
        *extra,
        "--",
        "-D",
        "warnings",
    ])


if __name__ == "__main__":
    sys.exit(main())
