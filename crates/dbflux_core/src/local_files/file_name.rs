use std::collections::HashSet;
use std::fmt;
use std::io;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};

/// Longest file name, in bytes, a [`SafeFileName`] holds. Most file systems
/// stop at 255 bytes; the margin leaves room for a collision suffix and for
/// the extension a caller appends.
pub const MAX_FILE_NAME_BYTES: usize = 200;

/// An extension longer than this is part of the stem: it is too long to be
/// one, and keeping it whole could leave no room for the name.
const MAX_EXTENSION_BYTES: usize = 16;

const FALLBACK_NAME: &str = "unnamed";

/// A single path component that is safe to create on Linux, macOS and
/// Windows, made from a name nobody on this machine chose.
///
/// Built only by [`SafeFileName::new`], so a function that takes one cannot
/// be handed a raw server name by mistake.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SafeFileName(String);

impl SafeFileName {
    /// `untrusted` as a file name:
    ///
    /// - `/`, `\`, the other characters Windows refuses (`: * ? " < > |`) and
    ///   control characters become `_`;
    /// - invisible formatting characters (bidirectional overrides, zero-width
    ///   characters) are dropped, so a name cannot hide its real extension;
    /// - runs of dots become one dot, so no `..` is left;
    /// - leading dots and spaces are dropped (no hidden files, no `.`), and
    ///   trailing dots and spaces too, which Windows would strip;
    /// - a name Windows reserves (`CON`, `NUL`, `COM1`, `LPT1`, ... with any
    ///   extension) gets a leading `_`;
    /// - the result is cut to [`MAX_FILE_NAME_BYTES`], keeping the extension;
    /// - nothing left becomes `unnamed`.
    pub fn new(untrusted: &str) -> Self {
        let replaced = replace_unsafe_characters(untrusted);
        let collapsed = collapse_dot_runs(&replaced);
        let trimmed = trim_edges(&collapsed);
        let name = if trimmed.is_empty() {
            FALLBACK_NAME.to_string()
        } else {
            trimmed.to_string()
        };

        Self(truncate_keeping_extension(&escape_reserved(name)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// This name with `-{number}` before its extension, such as
    /// `users-2.csv`, still within [`MAX_FILE_NAME_BYTES`].
    pub fn numbered(&self, number: u32) -> Self {
        let (stem, extension) = split_extension(&self.0);
        let suffix = format!("-{number}");
        let extension_len = extension.map_or(0, |extension| extension.len() + 1);
        let stem_budget = MAX_FILE_NAME_BYTES.saturating_sub(suffix.len() + extension_len);

        let mut name = truncate_at_char_boundary(stem, stem_budget).to_string();
        name.push_str(&suffix);
        if let Some(extension) = extension {
            name.push('.');
            name.push_str(extension);
        }

        Self(name)
    }
}

impl Deref for SafeFileName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<Path> for SafeFileName {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl fmt::Display for SafeFileName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Names handed out for one directory, so two server names that sanitize to
/// the same file name (`a/b` and `a_b`, or `Users` and `users`) do not
/// overwrite each other.
#[derive(Debug, Default)]
pub struct UniqueFileNames {
    taken: HashSet<String>,
}

impl UniqueFileNames {
    pub fn new() -> Self {
        Self::default()
    }

    /// `name`, or its first [`SafeFileName::numbered`] form not handed out
    /// yet. Names are compared without case, as macOS and Windows file
    /// systems compare them.
    pub fn claim(&mut self, name: SafeFileName) -> SafeFileName {
        let mut candidate = name.clone();
        let mut number = 2;

        while !self.taken.insert(candidate.as_str().to_lowercase()) {
            candidate = name.numbered(number);
            number += 1;
        }

        candidate
    }
}

/// `directory` joined with `name`, refused unless the result is a direct
/// child of `directory`. A sanitized name always is; the check stays so a
/// change to the sanitizer cannot quietly reopen the hole.
pub fn confined_join(directory: &Path, name: &SafeFileName) -> io::Result<PathBuf> {
    let path = directory.join(name.as_str());

    if is_single_normal_component(name.as_str()) && path.parent() == Some(directory) {
        Ok(path)
    } else {
        Err(outside_directory_error(name.as_str(), directory))
    }
}

/// The existing regular file `name` names inside `directory`, for a file
/// name read from a bundle someone else may have written. Refuses a name
/// with a separator, `..` or a root, and a symbolic link, which could point
/// at any file the user can read.
pub fn confined_existing_file(directory: &Path, name: &str) -> io::Result<PathBuf> {
    let plain = is_single_normal_component(name) && !name.contains(['/', '\\', '\0']);
    let path = directory.join(name);
    if !plain || path.parent() != Some(directory) {
        return Err(outside_directory_error(name, directory));
    }

    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} is not a regular file inside {}",
                name,
                directory.display()
            ),
        ));
    }

    Ok(path)
}

fn is_single_normal_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn outside_directory_error(name: &str, directory: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("'{name}' is not a file name inside {}", directory.display()),
    )
}

fn replace_unsafe_characters(name: &str) -> String {
    name.chars()
        .filter(|character| !is_invisible_format(*character))
        .map(
            |character| {
                if is_unsafe(character) { '_' } else { character }
            },
        )
        .collect()
}

fn is_unsafe(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
        )
}

fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

fn collapse_dot_runs(name: &str) -> String {
    let mut collapsed = String::with_capacity(name.len());
    let mut previous_was_dot = false;

    for character in name.chars() {
        let is_dot = character == '.';
        if !(is_dot && previous_was_dot) {
            collapsed.push(character);
        }
        previous_was_dot = is_dot;
    }

    collapsed
}

fn trim_edges(name: &str) -> &str {
    let is_edge = |character: char| character == '.' || character.is_whitespace();
    name.trim_start_matches(is_edge).trim_end_matches(is_edge)
}

/// Windows refuses these device names as a file's stem whatever follows the
/// first dot, so `nul.csv` cannot be created there.
fn escape_reserved(name: String) -> String {
    let stem = name.split('.').next().unwrap_or_default().trim_end();

    if is_reserved_on_windows(stem) {
        format!("_{name}")
    } else {
        name
    }
}

fn is_reserved_on_windows(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }

    let (Some(prefix), Some(number)) = (upper.get(..3), upper.get(3..)) else {
        return false;
    };
    let numbered_device = matches!(prefix, "COM" | "LPT");
    let device_number = matches!(
        number,
        "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
    );

    numbered_device && device_number
}

fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty()
                && !extension.is_empty()
                && extension.len() <= MAX_EXTENSION_BYTES =>
        {
            (stem, Some(extension))
        }
        _ => (name, None),
    }
}

fn truncate_keeping_extension(name: &str) -> String {
    if name.len() <= MAX_FILE_NAME_BYTES {
        return name.to_string();
    }

    let (stem, extension) = split_extension(name);
    match extension {
        Some(extension) => {
            let stem_budget = MAX_FILE_NAME_BYTES - extension.len() - 1;
            let stem = truncate_at_char_boundary(stem, stem_budget);
            format!("{}.{extension}", stem.trim_end_matches(['.', ' ']))
        }
        None => truncate_at_char_boundary(name, MAX_FILE_NAME_BYTES)
            .trim_end_matches(['.', ' '])
            .to_string(),
    }
}

fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
}
