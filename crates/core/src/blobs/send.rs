use std::{
    collections::HashSet,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use super::error::{BlobError, BlobTextError, Result};
use super::util::import_files;
use iroh::{Endpoint, protocol::Router};
use iroh_blobs::{
    ALPN, BlobFormat, BlobsProtocol, api::TempTag, format::collection::Collection,
    store::fs::FsStore, ticket::BlobTicket,
};
use tracing::trace;

/// Strategy for serving prepared blobs to the remote peer.
///
/// **Internal** (default): `BlobService::register` spawns its own
/// `iroh::protocol::Router` on a clone of the caller's `Endpoint`.  That
/// router is the sole accept-loop on that endpoint, which is fine when the
/// sender owns a dedicated endpoint.  Used by the CLI and by any caller that
/// hasn't wired a shared accept loop.
///
/// **External**: the caller already has a process-wide accept loop that
/// multiplexes ALPNs (e.g. the app crate's `BlobDispatcher` plugged into the
/// receiver service's `Router`).  In that mode we do *not* spawn another
/// router — we hand the prepared `BlobsProtocol` to the caller-provided
/// registrar, and the existing accept loop dispatches `iroh_blobs::ALPN`
/// connections to it.  Avoids the "two routers fighting for `endpoint.accept`"
/// race and, more importantly, avoids the "two endpoints with the same
/// secret key fighting for the relay slot" failure mode.
#[derive(Clone)]
pub enum BlobServingStrategy {
    Internal,
    External(Arc<dyn ExternalBlobRegistrar>),
}

impl std::fmt::Debug for BlobServingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Internal => f.write_str("BlobServingStrategy::Internal"),
            Self::External(_) => f.write_str("BlobServingStrategy::External(..)"),
        }
    }
}

impl Default for BlobServingStrategy {
    fn default() -> Self {
        Self::Internal
    }
}

/// Caller-provided hook for the **External** [`BlobServingStrategy`].
///
/// The implementer is responsible for routing inbound `iroh_blobs::ALPN`
/// connections on the shared endpoint to the protocol handler registered
/// here.  The contract:
/// - `register_blob_protocol` is called once before the sender writes its
///   `BlobTicket` to the peer.  After this returns Ok, the dispatcher must
///   be able to serve `iroh_blobs::ALPN` connections referencing the
///   collection hash inside `protocol`.
/// - `unregister_blob_protocol` is called exactly once after the transfer
///   finishes (success or failure).  It MUST clear the registration so the
///   next send can install its own protocol.
///
/// Trait object–safe because methods return boxed futures.
pub trait ExternalBlobRegistrar: std::fmt::Debug + Send + Sync + 'static {
    fn register_blob_protocol(
        &self,
        protocol: BlobsProtocol,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    fn unregister_blob_protocol(
        &self,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

#[derive(Debug)]
pub(crate) struct PreparedStore {
    store: FsStore,
    collection_tag: TempTag,
    files: Vec<PreparedFile>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedFile {
    pub(crate) path: String,
    pub(crate) size: u64,
}

impl PreparedStore {
    pub(crate) async fn prepare(root_dir: &Path, files: Vec<PathBuf>) -> Result<Self> {
        let store = FsStore::load(root_dir)
            .await
            .map_err(|source| BlobError::store_load(root_dir.to_path_buf(), source))?;

        let mut collection = Collection::default();
        let mut seen_transfer_paths = HashSet::new();
        let mut files_out = Vec::new();
        for path in files {
            trace!(input_path = %path.display(), "processing import input path");
            let imported = import_files(&store, path.clone()).await.map_err(|source| {
                BlobError::import_files(
                    path.display().to_string(),
                    BlobTextError::new(format!("{source:#}")),
                )
            })?;
            for file in imported {
                let transfer_path = file.transfer_path.clone();
                if !seen_transfer_paths.insert(transfer_path.clone()) {
                    return Err(BlobError::duplicate_transfer_path(transfer_path));
                }
                collection.extend([(transfer_path.clone(), file.temp_tag.hash())]);
                files_out.push(PreparedFile {
                    path: transfer_path,
                    size: file.size_bytes,
                });
            }
        }

        files_out.sort_by(|left, right| left.path.cmp(&right.path));

        let collection_tag = collection
            .store(store.as_ref())
            .await
            .map_err(|source| BlobError::store_collection(source))?;
        trace!(
            collection_hash = %collection_tag.hash(),
            item_count = seen_transfer_paths.len(),
            "stored collection in blob store"
        );

        Ok(Self {
            store,
            collection_tag,
            files: files_out,
        })
    }

    pub(crate) fn store(&self) -> &FsStore {
        &self.store
    }

    pub(crate) fn collection_tag(&self) -> &TempTag {
        &self.collection_tag
    }

    pub(crate) fn collection_hash(&self) -> iroh_blobs::Hash {
        self.collection_tag.hash()
    }

    pub(crate) fn manifest(&self) -> crate::protocol::message::TransferManifest {
        crate::protocol::message::TransferManifest {
            items: self
                .files
                .iter()
                .map(|file| crate::protocol::message::ManifestItem::File {
                    path: file.path.clone(),
                    size: file.size,
                })
                .collect(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct BlobService {
    endpoint: Endpoint,
}

#[derive(Debug)]
pub(crate) struct BlobRegistration {
    _prepared: PreparedStore,
    inner: BlobRegistrationInner,
    ticket: BlobTicket,
}

#[derive(Debug)]
enum BlobRegistrationInner {
    /// We spawned a dedicated `iroh::protocol::Router` on a clone of the
    /// sender's endpoint.  Shutdown tears the router down.
    InternalRouter(Router),
    /// The caller's accept loop will dispatch `iroh_blobs::ALPN`
    /// connections to `protocol` for us.  Shutdown asks the registrar to
    /// drop its reference so the next send can install a different protocol.
    ///
    /// We hold `protocol` here only to keep the `Arc<BlobsInner>` alive
    /// until shutdown — the registrar already has its own clone.
    External {
        registrar: Arc<dyn ExternalBlobRegistrar>,
        _protocol: BlobsProtocol,
    },
}

impl BlobService {
    pub(crate) fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    /// Register `prepared` for serving using the supplied strategy.  See
    /// [`BlobServingStrategy`] for the trade-offs.
    pub(crate) async fn register_with_strategy(
        self,
        prepared: PreparedStore,
        strategy: &BlobServingStrategy,
    ) -> Result<BlobRegistration> {
        let protocol = BlobsProtocol::new(prepared.store().as_ref(), None);
        let ticket = BlobTicket::new(
            self.endpoint.addr(),
            prepared.collection_tag().hash(),
            BlobFormat::HashSeq,
        );

        let inner = match strategy {
            BlobServingStrategy::Internal => {
                tracing::debug!(
                    target: "drift_core::blobs::send",
                    "registering blob protocol via Internal Router (sender owns dedicated endpoint)"
                );
                let router = Router::builder(self.endpoint)
                    .accept(ALPN, protocol)
                    .spawn();
                BlobRegistrationInner::InternalRouter(router)
            }
            BlobServingStrategy::External(registrar) => {
                tracing::debug!(
                    target: "drift_core::blobs::send",
                    "registering blob protocol via External registrar (sharing endpoint with another subsystem)"
                );
                registrar.register_blob_protocol(protocol.clone()).await?;
                BlobRegistrationInner::External {
                    registrar: Arc::clone(registrar),
                    _protocol: protocol,
                }
            }
        };

        Ok(BlobRegistration {
            _prepared: prepared,
            inner,
            ticket,
        })
    }
}

impl BlobRegistration {
    pub(crate) fn ticket(&self) -> &BlobTicket {
        &self.ticket
    }

    pub(crate) async fn shutdown(self) -> Result<()> {
        match self.inner {
            BlobRegistrationInner::InternalRouter(router) => {
                router
                    .shutdown()
                    .await
                    .map_err(|source| BlobError::store_shutdown("blob registration", source))?;
            }
            BlobRegistrationInner::External { registrar, _protocol } => {
                registrar.unregister_blob_protocol().await;
                drop(_protocol);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::blobs::error::BlobError;

    use super::PreparedStore;

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
    fn unique_temp_dir(prefix: &str) -> PathBuf {
        static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "{}-{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        );
        std::env::temp_dir().join(unique)
    }

    #[tokio::test]
    async fn prepare_store_rejects_duplicate_transfer_paths() -> Result<()> {
        let root = unique_temp_dir("drift-one-shot-duplicate-paths");
        let source = root.join("source");
        let store_root = root.join("store");
        std::fs::create_dir_all(&source)?;
        std::fs::create_dir_all(&store_root)?;
        std::fs::write(source.join("same.txt"), b"same")?;

        let err = PreparedStore::prepare(&store_root, vec![source.clone(), source])
            .await
            .expect_err("expected duplicate transfer path failure");
        let err_text = format!("{err:#}");
        assert!(err_text.contains("duplicate transfer path in manifest: source/same.txt"));

        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    /// Regression: when the caller selects [`BlobServingStrategy::External`],
    /// `register_with_strategy` must hand the freshly-built `BlobsProtocol`
    /// to the supplied registrar and `BlobRegistration::shutdown` must call
    /// `unregister_blob_protocol`.  Both ordering and pairing matter — a
    /// missed `unregister` would leak the active-blob slot for the next
    /// send.
    #[tokio::test]
    async fn external_strategy_invokes_register_then_unregister_in_order() -> Result<()> {
        use std::sync::atomic::AtomicUsize;
        use std::sync::{Arc, Mutex};

        use iroh::{Endpoint, SecretKey};
        use iroh_blobs::BlobsProtocol;

        use super::{BlobService, BlobServingStrategy, ExternalBlobRegistrar};
        use crate::blobs::error::Result as BlobResult;

        #[derive(Debug, Default)]
        struct RecordingRegistrar {
            // Track call ordering across the two methods so the test can
            // assert "register before unregister" rather than just call
            // counts.
            call_log: Mutex<Vec<&'static str>>,
            register_count: AtomicUsize,
            unregister_count: AtomicUsize,
            // Hold the registered protocol so we can verify it was actually
            // passed in (the dispatcher in production stores it the same way).
            stored_protocol: Mutex<Option<BlobsProtocol>>,
        }

        impl ExternalBlobRegistrar for RecordingRegistrar {
            fn register_blob_protocol(
                &self,
                protocol: BlobsProtocol,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = BlobResult<()>> + Send + '_>,
            > {
                Box::pin(async move {
                    self.call_log.lock().unwrap().push("register");
                    self.register_count
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    *self.stored_protocol.lock().unwrap() = Some(protocol);
                    Ok(())
                })
            }

            fn unregister_blob_protocol(
                &self,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>
            {
                Box::pin(async move {
                    self.call_log.lock().unwrap().push("unregister");
                    self.unregister_count
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    *self.stored_protocol.lock().unwrap() = None;
                })
            }
        }

        let root = unique_temp_dir("drift-blob-external-strategy");
        let source = root.join("source");
        let store_root = root.join("store");
        std::fs::create_dir_all(&source)?;
        std::fs::create_dir_all(&store_root)?;
        std::fs::write(source.join("hello.txt"), b"world")?;
        let prepared = PreparedStore::prepare(&store_root, vec![source]).await?;

        let endpoint = match Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(SecretKey::from_bytes(&[3u8; 32]))
            .bind()
            .await
        {
            Ok(endpoint) => endpoint,
            Err(_) => {
                // No UDP socket available in this sandbox — same gate as
                // the receiver-side tests use.  Skip rather than fail.
                std::fs::remove_dir_all(&root)?;
                return Ok(());
            }
        };

        let registrar = Arc::new(RecordingRegistrar::default());
        let strategy = BlobServingStrategy::External(Arc::clone(&registrar)
            as Arc<dyn ExternalBlobRegistrar>);
        let service = BlobService::new(endpoint.clone());
        let registration = service.register_with_strategy(prepared, &strategy).await?;

        assert_eq!(
            registrar.register_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "register_blob_protocol must be called exactly once"
        );
        assert_eq!(
            registrar.unregister_count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "unregister_blob_protocol must not be called during registration"
        );
        assert!(
            registrar.stored_protocol.lock().unwrap().is_some(),
            "registrar must have received the BlobsProtocol instance"
        );

        registration.shutdown().await?;

        assert_eq!(
            registrar.unregister_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "unregister_blob_protocol must be called exactly once on shutdown"
        );
        assert_eq!(
            registrar.call_log.lock().unwrap().as_slice(),
            &["register", "unregister"],
            "register must always precede unregister"
        );
        assert!(
            registrar.stored_protocol.lock().unwrap().is_none(),
            "shutdown must clear the registered protocol"
        );

        endpoint.close().await;
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepare_store_rejects_nested_symbolic_links() -> Result<()> {
        let root = unique_temp_dir("drift-one-shot-symlink-entry");
        let source = root.join("source");
        let store_root = root.join("store");
        std::fs::create_dir_all(&source)?;
        std::fs::create_dir_all(&store_root)?;
        std::fs::write(source.join("real.txt"), b"real")?;
        symlink("real.txt", source.join("link.txt"))?;

        let err = PreparedStore::prepare(&store_root, vec![source])
            .await
            .expect_err("expected nested symbolic link to be rejected");
        match err {
            BlobError::ImportFiles { .. } => {}
            other => panic!("unexpected error: {other:#}"),
        }

        std::fs::remove_dir_all(&root)?;
        Ok(())
    }
}
