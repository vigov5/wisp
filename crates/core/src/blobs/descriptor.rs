//! Reading a descriptor source through its descriptor, not through its path.
//!
//! Android hands a picked or shared file over as `/proc/self/fd/<n>`: a magic
//! link onto a descriptor the app is holding open. The blob store references
//! that path rather than copying the file, and so has to read it repeatedly —
//! but reopening the link is a fresh path walk that lands in MediaProvider's
//! FUSE daemon, which re-checks permission against our own uid and refuses.
//! The grant covers the descriptor we were given, not a second open by name.
//! Measured on Pixel 7 / Android 17: `EACCES` for every provider tried, so on
//! current Android *every* external-storage source fails that way.
//!
//! The descriptor itself never stops being readable. So instead of letting the
//! store open the path, we duplicate the descriptor — which needs no path walk
//! and so no permission check — and register the handle for that path. See
//! `third_party/iroh-blobs/PATCH.md`.
//!
//! The registration has to outlive the import: the store reads the file again
//! whenever it serves bytes. [`DescriptorHandles`] holds them for as long as
//! the prepared store exists and releases them with it.

use std::path::Path;

#[cfg(unix)]
use std::{fs::File, os::fd::BorrowedFd, path::PathBuf};

#[cfg(unix)]
use tracing::{trace, warn};

/// The descriptor paths registered with the blob store, released on drop.
#[derive(Debug, Default)]
pub(crate) struct DescriptorHandles {
    #[cfg(unix)]
    paths: Vec<PathBuf>,
}

impl DescriptorHandles {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers `path` so the store reads it through a duplicate of the
    /// descriptor it names.
    ///
    /// Silently does nothing where there is nothing to do — a path that is not
    /// a descriptor path, or a platform without one — because the store opening
    /// the path itself is the correct behaviour everywhere else.
    #[cfg(unix)]
    pub(crate) fn register(&mut self, path: &Path) {
        let Some(fd) = descriptor_number(path) else {
            return;
        };
        // Our own descriptor, held open by the caller for the whole transfer.
        // `dup` copies the entry in our file table; it never resolves a name,
        // which is exactly why it works where opening the path does not.
        let file = match unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned() {
            Ok(owned) => File::from(owned),
            Err(source) => {
                // Nothing is broken yet: the store will try the path, and fail
                // there if it must.
                warn!(path = %path.display(), %source, "could not duplicate descriptor");
                return;
            }
        };
        trace!(path = %path.display(), "reading descriptor source through its descriptor");
        iroh_blobs::store::fs::preopened::register(path.to_path_buf(), file);
        self.paths.push(path.to_path_buf());
    }

    #[cfg(not(unix))]
    pub(crate) fn register(&mut self, _path: &Path) {}
}

/// Metadata for `path`, read through its descriptor where it has one.
///
/// `stat` on a descriptor path is a path walk like any other, so it is subject
/// to the same refusal as opening it; `fstat` on the descriptor is not. Falls
/// through to the ordinary path for everything else.
pub(crate) fn metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    #[cfg(unix)]
    if iroh_blobs::store::fs::preopened::is_registered(path) {
        return iroh_blobs::store::fs::preopened::open(path)?.metadata();
    }
    std::fs::metadata(path)
}

impl Drop for DescriptorHandles {
    fn drop(&mut self) {
        #[cfg(unix)]
        for path in self.paths.drain(..) {
            iroh_blobs::store::fs::preopened::unregister(&path);
        }
    }
}

/// The `<n>` of a `/proc/self/fd/<n>` path, or None for any other path.
///
/// Deliberately exact: only this shape is a descriptor we opened ourselves, and
/// duplicating an arbitrary number would read whatever unrelated file happens
/// to sit at that slot.
#[cfg(unix)]
fn descriptor_number(path: &Path) -> Option<std::os::fd::RawFd> {
    path.to_str()?
        .strip_prefix("/proc/self/fd/")?
        .parse::<std::os::fd::RawFd>()
        .ok()
        .filter(|fd| *fd >= 0)
}

#[cfg(test)]
mod tests {
    // Everything worth testing here is descriptor handling, which only exists
    // on unix; on Windows this module is deliberately empty.
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    #[test]
    fn only_descriptor_paths_are_recognised() {
        use std::path::PathBuf;

        assert_eq!(
            descriptor_number(&PathBuf::from("/proc/self/fd/42")),
            Some(42)
        );
        assert_eq!(
            descriptor_number(&PathBuf::from("/proc/self/fd/0")),
            Some(0)
        );
        // Anything else is an ordinary path the store should open itself.
        assert_eq!(
            descriptor_number(&PathBuf::from("/home/me/holiday.mp4")),
            None
        );
        assert_eq!(descriptor_number(&PathBuf::from("/proc/self/fd/")), None);
        assert_eq!(descriptor_number(&PathBuf::from("/proc/self/fd/-1")), None);
        assert_eq!(descriptor_number(&PathBuf::from("/proc/self/fd/1/x")), None);
        assert_eq!(descriptor_number(&PathBuf::from("/proc/1234/fd/7")), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_registered_descriptor_reads_and_is_released_on_drop() -> std::io::Result<()> {
        use std::io::Read;
        use std::os::fd::AsRawFd;

        let dir = std::env::temp_dir().join("wisp-descriptor-handles");
        std::fs::create_dir_all(&dir)?;
        let backing = dir.join("backing.bin");
        std::fs::write(&backing, b"holiday")?;
        let held = File::open(&backing)?;
        let fd_path = PathBuf::from(format!("/proc/self/fd/{}", held.as_raw_fd()));
        if !fd_path.exists() {
            // No procfs (macOS): nothing to assert.
            let _ = std::fs::remove_dir_all(&dir);
            return Ok(());
        }

        {
            let mut handles = DescriptorHandles::new();
            handles.register(&fd_path);
            let mut file = iroh_blobs::store::fs::preopened::open(&fd_path)?;
            let mut read = String::new();
            file.read_to_string(&mut read)?;
            assert_eq!(read, "holiday");
        }
        // Dropped with the handles, so the store would go back to the path.
        assert!(!iroh_blobs::store::fs::preopened::is_registered(&fd_path));

        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
