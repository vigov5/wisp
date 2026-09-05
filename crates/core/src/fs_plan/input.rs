use std::path::{Path, PathBuf};

use crate::transfer::path::{TransferPathError, input_root_name, validate_transfer_path};

/// Upper bound on a carried transfer path, so a hostile or buggy provider
/// cannot make the receiver create an unbounded directory chain.
const MAX_TRANSFER_PATH_SEGMENTS: usize = 32;

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
    /// `transfer_path` is where the file lands on the receiver. It is a bare
    /// file name for a picked file, and a relative path (`photos/trip/cat.jpg`)
    /// for one file of a picked folder — a folder is sent as one descriptor per
    /// file, since a SAF tree offers no single handle to open.
    ///
    /// The descriptor must stay open for the whole transfer, not just the
    /// import: the blob store references the file in place
    /// ([`iroh_blobs::api::blobs::ImportMode::TryReference`]) and reopens
    /// this path lazily every time it serves bytes.
    FileDescriptor {
        path: PathBuf,
        transfer_path: String,
    },
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

    /// Where this source lands on the receiver: the final path component for a
    /// [`SendInput::Path`] (its tree is walked from there), the carried
    /// relative path for a descriptor.
    pub fn transfer_path(&self) -> Result<String, TransferPathError> {
        match self {
            Self::Path(path) => input_root_name(path),
            Self::FileDescriptor { transfer_path, .. } => {
                validate_carried_transfer_path(transfer_path)?;
                Ok(transfer_path.clone())
            }
        }
    }
}

impl From<PathBuf> for SendInput {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

/// A carried path becomes a transfer path verbatim, so it gets the same
/// treatment the walk gives a discovered one: relative, `/`-separated, no `..`
/// or empty segments — and bounded in depth, since nothing upstream limits it.
fn validate_carried_transfer_path(path: &str) -> Result<(), TransferPathError> {
    let segments = validate_transfer_path(path)?;
    if segments.len() > MAX_TRANSFER_PATH_SEGMENTS {
        return Err(TransferPathError::InvalidSegment);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(transfer_path: &str) -> SendInput {
        SendInput::FileDescriptor {
            path: PathBuf::from("/proc/self/fd/7"),
            transfer_path: transfer_path.to_owned(),
        }
    }

    #[test]
    fn path_input_takes_its_name_from_the_final_component() {
        let input = SendInput::from(PathBuf::from("/home/u/holiday.mp4"));
        assert_eq!(input.transfer_path().expect("name"), "holiday.mp4");
        assert!(!input.is_file_descriptor());
    }

    #[test]
    fn descriptor_input_uses_the_carried_path_not_the_fd_number() {
        let input = descriptor("holiday.mp4");
        assert_eq!(input.transfer_path().expect("name"), "holiday.mp4");
        assert_eq!(input.path(), Path::new("/proc/self/fd/7"));
        assert!(input.is_file_descriptor());
    }

    /// One file of a picked folder arrives as its own descriptor, so the
    /// carried path has to keep the folder structure.
    #[test]
    fn descriptor_input_accepts_a_nested_transfer_path() {
        assert_eq!(
            descriptor("photos/trip/cat.jpg")
                .transfer_path()
                .expect("nested path"),
            "photos/trip/cat.jpg"
        );
    }

    #[test]
    fn descriptor_path_may_not_escape_the_transfer_root() {
        let too_deep = vec!["d"; MAX_TRANSFER_PATH_SEGMENTS + 1].join("/");
        for path in [
            "", "..", ".", "a/../b", r"a\b", "/abs", "sub/", "a//b", &too_deep,
        ] {
            descriptor(path)
                .transfer_path()
                .expect_err(&format!("expected {path:?} to be rejected"));
        }
    }
}
