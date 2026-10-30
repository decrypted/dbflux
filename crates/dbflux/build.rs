//! Records the build stamp the main window title shows, and embeds the Windows
//! executable resources: the application icon and the `VERSIONINFO` block that
//! Explorer shows under Properties and that the taskbar, the Open With dialog,
//! and installers read the product name from.
//!
//! macOS takes the icon and version from the bundle and Linux from the desktop
//! entry, so on those platforms the script only registers its inputs and exits.
//!
//! The resource text is assembled on every platform so `cargo check` on Linux
//! or macOS still type-checks it; only the resource compiler call is gated.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Workspace paths, relative to its root, that the binary is compiled from.
///
/// Cargo has no directive for "rerun when a dependency crate is rebuilt", so
/// the stamp watches the sources of every workspace crate the binary links
/// instead: a change that relinks the binary almost always touches one of
/// these. Changes it misses, such as an environment variable that only a
/// dependency's build script reads, relink with the previous stamp. Watching
/// them keeps a no-op build a no-op, where an always-dirty build script would
/// relink the binary (a thin-LTO link in release) on every `cargo build`. A new
/// top-level directory that a crate embeds with `include_bytes!` belongs on
/// this list.
const STAMPED_INPUTS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "crates",
    "vendor",
    "resources",
    "CHANGELOG.md",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env_var("CARGO_MANIFEST_DIR"));
    emit_build_stamp(&manifest_dir.join("../.."));

    let version = env_var("CARGO_PKG_VERSION");
    let identity = ChannelIdentity::from_version(&version);

    let icon = manifest_dir
        .join("../../packaging/icons")
        .join(identity.icon_file);
    println!("cargo:rerun-if-changed={}", icon.display());

    let out_dir = PathBuf::from(env_var("OUT_DIR"));
    let script = out_dir.join("dbflux.rc");
    let resource = ResourceScript::new(&icon, &version, identity).render();

    #[expect(
        clippy::panic,
        reason = "build scripts signal failure by aborting: an unwritable \
                  resource script must fail the build, not be skipped"
    )]
    if let Err(error) = std::fs::write(&script, resource) {
        panic!("failed to write {}: {error}", script.display());
    }

    #[cfg(windows)]
    embed(&script, &icon);

    #[cfg(not(windows))]
    drop(script);
}

/// Passes the build time and the commit to the binary as `DBFLUX_BUILD_UNIX_TIME`
/// and `DBFLUX_BUILD_GIT_SHA` (empty when the tree is not a git checkout).
fn emit_build_stamp(workspace_root: &Path) {
    for input in STAMPED_INPUTS {
        watch_if_present(&workspace_root.join(input));
    }

    // Reproducible builds pin the time through this variable, so honoring it
    // keeps such a binary byte-identical across rebuilds.
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let built_at = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(unix_time_now);
    println!("cargo:rustc-env=DBFLUX_BUILD_UNIX_TIME={built_at}");

    let git_sha = git_short_sha(workspace_root).unwrap_or_default();
    println!("cargo:rustc-env=DBFLUX_BUILD_GIT_SHA={git_sha}");
}

/// Cargo treats a watched path that does not exist as changed on every build,
/// which would relink the binary each time, so only existing paths are watched.
fn watch_if_present(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn unix_time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The abbreviated commit of the workspace checkout, if it is one.
///
/// Builds also run from copies of the tree without `.git` (an rsync to a build
/// machine), and a missing `git` binary is just as possible, so every failure
/// means "no commit to show" rather than a failed build.
fn git_short_sha(workspace_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args([
            "rev-parse",
            "--show-toplevel",
            "--git-path",
            "HEAD",
            "--git-path",
            "logs/HEAD",
            "--short=8",
            "HEAD",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut lines = stdout.lines();
    let toplevel = lines.next()?;
    let head = lines.next()?;
    let head_log = lines.next()?;
    let sha = lines.next()?.trim();

    // git searches parent directories, so a copy without `.git` that sits
    // inside another repository would otherwise report that repository's
    // commit.
    let same_tree =
        std::fs::canonicalize(toplevel).ok()? == std::fs::canonicalize(workspace_root).ok()?;
    if !same_tree || sha.is_empty() {
        return None;
    }

    // HEAD moves on checkout and the reflog grows on every commit, reset and
    // checkout, so watching both refreshes the commit without a source change.
    // `--git-path` answers relative to the workspace root in a plain checkout
    // and absolutely in a linked worktree.
    for git_path in [head, head_log] {
        watch_if_present(&workspace_root.join(git_path));
    }

    Some(sha.to_string())
}

/// Reads a Cargo-provided environment variable.
#[expect(
    clippy::panic,
    reason = "Cargo always sets the queried variables for build scripts, so a \
              missing one means a broken build environment that must abort the \
              build"
)]
fn env_var(name: &str) -> String {
    match env::var(name) {
        Ok(value) => value,
        Err(error) => panic!("{name} is not set for the build script: {error}"),
    }
}

/// Compile the resource script and link it into the executable.
///
/// `manifest_required` fails the build when no resource compiler is available
/// or compilation fails. An executable that silently shipped without its icon
/// and version block would be the worse outcome.
#[cfg(windows)]
#[expect(
    clippy::panic,
    reason = "missing application icon or failed resource compilation are fatal \
              Windows build errors; preserve their existing diagnostics"
)]
fn embed(script: &Path, icon: &Path) {
    if !icon.is_file() {
        panic!("application icon not found at {}", icon.display());
    }

    if let Err(result) = embed_resource::compile(script, embed_resource::NONE).manifest_required() {
        panic!("failed to embed the Windows resources: {result}");
    }
}

/// The per-channel identity the executable carries.
///
/// Mirrors `ReleaseChannel::from_version` in `dbflux_core`. The build script
/// cannot depend on that crate without pulling the whole workspace into the
/// build-dependency graph, so the three-line rule is repeated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChannelIdentity {
    product_name: &'static str,
    icon_file: &'static str,
    prerelease: bool,
}

impl ChannelIdentity {
    fn from_version(version: &str) -> Self {
        if version.contains("-nightly") {
            Self {
                product_name: "DBFlux Nightly",
                icon_file: "dbflux-nightly.ico",
                prerelease: true,
            }
        } else if version.contains("-rc") {
            Self {
                product_name: "DBFlux",
                icon_file: "dbflux.ico",
                prerelease: true,
            }
        } else {
            Self {
                product_name: "DBFlux",
                icon_file: "dbflux.ico",
                prerelease: false,
            }
        }
    }
}

/// The text of the `.rc` file handed to the resource compiler.
struct ResourceScript<'a> {
    icon: &'a Path,
    version: &'a str,
    identity: ChannelIdentity,
}

impl<'a> ResourceScript<'a> {
    fn new(icon: &'a Path, version: &'a str, identity: ChannelIdentity) -> Self {
        Self {
            icon,
            version,
            identity,
        }
    }

    fn render(&self) -> String {
        let [major, minor, patch] = numeric_version(self.version);
        let file_flags = if self.identity.prerelease {
            "0x2L"
        } else {
            "0x0L"
        };
        let company = authors_without_email(&env_var("CARGO_PKG_AUTHORS"));
        let description = env_var("CARGO_PKG_DESCRIPTION");

        // Resource scripts take C string literals, so a Windows path's
        // backslashes have to be escaped.
        let icon_literal = self.icon.display().to_string().replace('\\', "\\\\");

        let strings = [
            ("CompanyName", company.as_str()),
            ("FileDescription", description.as_str()),
            ("FileVersion", self.version),
            ("InternalName", "dbflux"),
            ("LegalCopyright", "MIT OR Apache-2.0"),
            ("OriginalFilename", "dbflux.exe"),
            ("ProductName", self.identity.product_name),
            ("ProductVersion", self.version),
        ];

        let string_values = strings
            .iter()
            .map(|(key, value)| format!("      VALUE \"{key}\", \"{}\"", escape_rc_string(value)))
            .collect::<Vec<_>>()
            .join("\n");

        // Resource id 1 is what the shell reads as the executable's icon. gpui
        // embeds its manifest under the same id but a different resource type,
        // so the two do not collide.
        format!(
            r#"1 ICON "{icon_literal}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS {file_flags}
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
{string_values}
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
        )
    }
}

/// `FILEVERSION` takes four 16-bit integers, so the pre-release and build
/// metadata of the semver string are dropped and the fourth component is
/// always zero. The full string survives in `FileVersion` and `ProductVersion`.
fn numeric_version(version: &str) -> [u16; 3] {
    let core = version.split(['-', '+']).next().unwrap_or(version);

    let mut parts = core.split('.').map(|part| part.parse::<u16>().unwrap_or(0));

    [
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    ]
}

/// `CARGO_PKG_AUTHORS` is `Name <email>:Name <email>`; Properties shows the
/// names only.
fn authors_without_email(authors: &str) -> String {
    authors
        .split(':')
        .map(|author| author.split('<').next().unwrap_or(author).trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn escape_rc_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\"\"")
}
