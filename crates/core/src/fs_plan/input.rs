use std::path::{Path, PathBuf};

use crate::transfer::path::{TransferPathError, input_root_name, validate_transfer_path};

/// One source the user picked for a send.
///
/// Desktop picks are plain paths.  Android SAF picks are not: scoped storage
/// hands the app a `content://` URI, and the only way to give the core
/// something openable without first copying the whole file into the app cache
/// is to open that URI and pass the live descriptor as `/proc/self/fd/<n>`.
/// Such a path carries no usable file name, so the name the receiver should
/// see has to travel alongside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendInput {
    /// A path the user selected directly.  Either a file or a directory;
    /// symlinks are rejected by the import walk.
    Path(PathBuf),
    /// A live file descriptor exposed at `path` (`/proc/self/fd/<n>`).
    ///
    /// Always a single regular file — never walked as a directory — and the
    /// magic symlink is followed rather than rejected: we opened the
    /// descriptor ourselves from a URI the user picked, so there is no
    /// untrusted link for the symlink check to guard against.
    ///
    /// The descriptor must stay open for the whole transfer, not just the
    /// import: the blob store references the file in place
    /// ([`iroh_blobs::api::blobs::ImportMode::TryReference`]) and reopens
    /// this path lazily every time it serves bytes.
    FileDescriptor { path: PathBuf, name: String },
}

impl SendInput {
    /// The path the OS opens for this source.
    pub fn path(&self) -> &Path {
        match self {
            Self::Path(path) => path,
            Self::FileDescriptor { path, .. } => path,
        }
    }

    /// Consumes the input and yields its path.
    pub fn into_path(self) -> PathBuf {
        match self {
            Self::Path(path) => path,
            Self::FileDescriptor { path, .. } => path,
        }
    }

    /// True when the path is a live descriptor rather than a durable
    /// filesystem location.  Callers use this to skip directory traversal and
    /// to follow the `/proc/self/fd` symlink instead of rejecting it.
    pub fn is_file_descriptor(&self) -> bool {
        matches!(self, Self::FileDescriptor { .. })
    }

    /// The name the receiver sees for this source: the final path component
    /// for a [`SendInput::Path`], the carried name for a descriptor.
    pub fn display_name(&self) -> Result<String, TransferPathError> {
        match self {
            Self::Path(path) => input_root_name(path),
            Self::FileDescriptor { name, .. } => {
                validate_file_name(name)?;
                Ok(name.clone())
            }
        }
    }
}

impl From<PathBuf> for SendInput {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

/// A carried name has to be exactly one safe path segment — it becomes a
/// transfer path root on the receiver, so `..`, separators and empties are all
/// out.
fn validate_file_name(name: &str) -> Result<(), TransferPathError> {
    let segments = validate_transfer_path(name)?;
    if segments.len() != 1 {
        return Err(TransferPathError::InvalidSegment);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(name: &str) -> SendInput {
        SendInput::FileDescriptor {
            path: PathBuf::from("/proc/self/fd/7"),
            name: name.to_owned(),
        }
    }

    #[test]
    fn path_input_takes_its_name_from_the_final_component() {
        let input = SendInput::from(PathBuf::from("/home/u/holiday.mp4"));
        assert_eq!(input.display_name().expect("name"), "holiday.mp4");
        assert!(!input.is_file_descriptor());
    }

    #[test]
    fn descriptor_input_uses_the_carried_name_not_the_fd_number() {
        let input = descriptor("holiday.mp4");
        assert_eq!(input.display_name().expect("name"), "holiday.mp4");
        assert_eq!(input.path(), Path::new("/proc/self/fd/7"));
        assert!(input.is_file_descriptor());
    }

    #[test]
    fn descriptor_name_may_not_escape_the_transfer_root() {
        for name in ["", "..", ".", "a/b", r"a\b", "/abs", "sub/"] {
            descriptor(name)
                .display_name()
                .expect_err(&format!("expected {name:?} to be rejected"));
        }
    }
}
