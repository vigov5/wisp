use crate::fs_plan::ConflictPolicy;
use crate::protocol::message::TransferManifest;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransferStatus {
    Transferring,
    Paused,
    DataComplete,
    Finalizing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferRecord {
    pub collection_hash: iroh_blobs::Hash,
    pub status: TransferStatus,
    pub output_dir: PathBuf,
    pub conflict_policy: ConflictPolicy,
    pub manifest: TransferManifest,
    #[serde(default)]
    pub bytes_received: u64,
    pub exported_files: HashSet<String>,
    pub created_at: std::time::SystemTime,
    pub updated_at: std::time::SystemTime,
}

impl TransferRecord {
    pub fn new(
        collection_hash: iroh_blobs::Hash,
        output_dir: PathBuf,
        conflict_policy: ConflictPolicy,
        manifest: TransferManifest,
    ) -> Self {
        let now = std::time::SystemTime::now();
        Self {
            collection_hash,
            status: TransferStatus::Transferring,
            output_dir,
            conflict_policy,
            manifest,
            bytes_received: 0,
            exported_files: HashSet::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let path = dir.join("record.json");
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let path = dir.join("record.json");
        let content = serde_json::to_vec(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Create the temporary file atomically in the record directory. A
        // random, create-new name avoids following an attacker-controlled
        // symlink, while the same-directory rename prevents readers from ever
        // observing a partially-written JSON document after a crash.
        let (temp_path, mut temp_file) = (0..16)
            .find_map(|_| {
                let temp_path = dir.join(format!(".record-{:016x}.tmp", rand::random::<u64>()));
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp_path)
                {
                    Ok(file) => Some(Ok((temp_path, file))),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .unwrap_or_else(|| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "could not allocate a unique transfer record temp file",
                ))
            })?;

        if let Err(error) = temp_file.write_all(&content) {
            drop(temp_file);
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        drop(temp_file);

        if let Err(error) = fs::rename(&temp_path, &path) {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::message::ManifestItem;

    #[test]
    fn record_roundtrips_json() {
        let dir = tempfile::tempdir().unwrap();
        let hash = [1u8; 32].into();
        let record = TransferRecord::new(
            hash,
            PathBuf::from("/tmp/out"),
            ConflictPolicy::Rename,
            TransferManifest {
                items: vec![ManifestItem::File {
                    path: "test.txt".to_owned(),
                    size: 10,
                }],
            },
        );

        record.save(dir.path()).unwrap();
        let loaded = TransferRecord::load(dir.path()).unwrap();
        assert_eq!(record.collection_hash, loaded.collection_hash);
        assert_eq!(record.manifest, loaded.manifest);
        assert_eq!(record.status, loaded.status);
        assert_eq!(loaded.bytes_received, 0);
    }

    #[test]
    fn record_load_defaults_missing_bytes_received_for_older_records() {
        let json = r#"{
          "collection_hash": "0101010101010101010101010101010101010101010101010101010101010101",
          "status": "Transferring",
          "output_dir": "/tmp/out",
          "conflict_policy": "Rename",
          "manifest": {
            "items": [
              { "type": "file", "path": "test.txt", "size": 10 }
            ]
          },
          "exported_files": [],
          "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
          "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 }
        }"#;

        let loaded: TransferRecord = serde_json::from_str(json).unwrap();

        assert_eq!(loaded.bytes_received, 0);
    }

    #[test]
    fn record_save_atomically_replaces_previous_value() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = TransferRecord::new(
            [2u8; 32].into(),
            PathBuf::from("/tmp/out"),
            ConflictPolicy::Rename,
            TransferManifest { items: Vec::new() },
        );

        record.bytes_received = 10;
        record.save(dir.path()).unwrap();
        record.bytes_received = 20;
        record.save(dir.path()).unwrap();

        assert_eq!(TransferRecord::load(dir.path()).unwrap().bytes_received, 20);
        assert_eq!(
            fs::read_dir(dir.path())
                .unwrap()
                .filter_map(std::result::Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0,
        );
    }
}
