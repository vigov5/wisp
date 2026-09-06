//! Files an embedder has already opened, for paths that cannot be opened again.
//!
//! `DataLocation::External` normally names a path and reopens it whenever the
//! bytes are needed. That assumes reopening is possible, which is not always
//! true: Android hands an app a `/proc/self/fd/<n>` magic link for a file it
//! granted access to, and reopening that link is a fresh path walk which lands
//! in the platform's FUSE daemon and is refused. The descriptor itself stays
//! perfectly readable — only the path is closed to us.
//!
//! So an embedder that holds such a descriptor can register it here against
//! the path it will import, and every open of that path inside the store uses
//! a duplicate of the live handle instead. Reads are positional
//! (`read_at`/`read_exact_at`), so sharing the file offset between duplicates
//! is not a problem.
//!
//! Registrations live until removed. They are process-wide rather than
//! per-store because the path is what identifies the data throughout the
//! store, and they are deliberately *not* persisted: a descriptor is only
//! meaningful to the process that holds it, so a store reopened later falls
//! back to opening the path and fails honestly rather than reading something
//! else.

use std::{
    collections::HashMap,
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

fn registry() -> &'static Mutex<HashMap<PathBuf, File>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, File>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Registers `file` as the way to read `path`, replacing any previous entry.
///
/// The handle is held until [`unregister`] is called, so the caller decides how
/// long the descriptor has to stay alive — which for a send is the whole
/// transfer, not just the import.
pub fn register(path: impl Into<PathBuf>, file: File) {
    registry()
        .lock()
        .unwrap()
        .insert(path.into(), file);
}

/// Drops the handle registered for `path`, returning it if there was one.
pub fn unregister(path: impl AsRef<Path>) -> Option<File> {
    registry().lock().unwrap().remove(path.as_ref())
}

/// Whether `path` has a registered handle.
pub fn is_registered(path: impl AsRef<Path>) -> bool {
    registry().lock().unwrap().contains_key(path.as_ref())
}

/// Opens `path` for reading, preferring a registered handle over the path
/// itself. Used everywhere the store would otherwise call `File::open` on a
/// path it does not own, and public so an embedder can check that what it
/// registered reads back.
pub fn open(path: &Path) -> io::Result<File> {
    if let Some(file) = registry().lock().unwrap().get(path) {
        return file.try_clone();
    }
    File::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_handle_is_used_instead_of_the_path() -> io::Result<()> {
        let dir = std::env::temp_dir().join("iroh-blobs-preopened-test");
        std::fs::create_dir_all(&dir)?;
        let real = dir.join("real.bin");
        std::fs::write(&real, b"holiday")?;

        // A path that does not exist, standing in for one that cannot be
        // opened: only the registered handle can serve it.
        let unopenable = dir.join("not-a-file.bin");
        assert!(open(&unopenable).is_err());

        register(unopenable.clone(), File::open(&real)?);
        let mut file = open(&unopenable)?;
        let mut read = String::new();
        std::io::Read::read_to_string(&mut file, &mut read)?;
        assert_eq!(read, "holiday");

        assert!(unregister(&unopenable).is_some());
        assert!(open(&unopenable).is_err());

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
