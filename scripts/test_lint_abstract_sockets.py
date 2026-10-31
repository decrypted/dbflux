"""Tests for the `abstract-sockets` check in lint.py; no cargo or Rust build needed."""
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

sys.path.insert(0, str(ROOT / "scripts"))
import lint  # noqa: E402


class AbstractSocketScanTests(unittest.TestCase):
    def scan(self, files):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative, text in files.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding="utf-8")
            return lint.abstract_socket_sites(root)

    def test_flags_namespaced_names_in_code_and_tests(self):
        sites = self.scan({
            "crates/dbflux_x/src/host.rs": (
                "use interprocess::local_socket::{GenericNamespaced, prelude::*};\n"
                "fn bind(id: &str) {\n"
                "    let name = id.to_ns_name::<GenericNamespaced>();\n"
                "}\n"
            ),
            "crates/dbflux_x/tests/it.rs": (
                "fn t() { let a = SocketAddr::from_abstract_name(b\"x\"); }\n"
            ),
            "examples/custom_x/src/main.rs": "let n = id.to_ns_name::<GenericNamespaced>();\n",
            "crates/dbflux_x/src/lib.rs": (
                "#[cfg(test)]\n"
                "mod tests {\n"
                "    fn t() { let _ = \"x\".to_ns_name::<AbstractNsUdSocket>(); }\n"
                "}\n"
            ),
        })

        self.assertEqual(
            sites,
            [
                ("crates/dbflux_x/src/host.rs", 1),
                ("crates/dbflux_x/src/host.rs", 3),
                ("crates/dbflux_x/src/lib.rs", 3),
                ("crates/dbflux_x/tests/it.rs", 1),
                ("examples/custom_x/src/main.rs", 1),
            ],
        )

    def test_the_socket_helper_comments_and_path_names_are_allowed(self):
        sites = self.scan({
            lint.ABSTRACT_SOCKET_HELPER: "let n = leaf.to_ns_name::<GenericNamespaced>();\n",
            "crates/dbflux_x/src/client.rs": (
                "// The old code used to_ns_name::<GenericNamespaced>().\n"
                "let name = path.to_fs_name::<GenericFilePath>(); // not GenericNamespaced\n"
                "let socket = dbflux_ipc::driver_socket(id)?;\n"
            ),
        })

        self.assertEqual(sites, [])

    def test_the_repository_has_no_namespaced_names_outside_the_helper(self):
        self.assertEqual(lint.abstract_socket_sites(), [])


if __name__ == "__main__":
    unittest.main()
