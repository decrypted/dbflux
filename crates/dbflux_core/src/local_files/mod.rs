//! Local files named after data that came from a server.
//!
//! A table, schema, collection or object name is chosen by whoever controls
//! the server or the bucket, so it is not a file name: it can hold `/`, `\`,
//! `..`, a drive prefix, control characters or a name Windows reserves, and
//! `Path::join` with an absolute name drops the directory it was joined to.
//! A file name built from such a name is a [`SafeFileName`], and the file is
//! placed with [`confined_join`], which refuses anything that would land
//! outside the chosen directory.

mod file_name;
mod private_dir;

pub use file_name::{
    MAX_FILE_NAME_BYTES, SafeFileName, UniqueFileNames, confined_existing_file, confined_join,
};
#[cfg(unix)]
pub use private_dir::effective_uid;
pub use private_dir::{create_private_file, private_dir_in};

#[cfg(test)]
mod tests;
