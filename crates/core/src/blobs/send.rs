use std::{
    collections::HashSet,
    future::Future,
    path::Path,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use super::error::{BlobError, BlobTextError, Result};
use super::lan_provider::LanBlobProvider;
use super::receive::BlobTransportProfile;
use super::telemetry::{
    BlobProviderTelemetry, TransferEnd, benchmark_run_id, is_enabled as telemetry_enabled,
};
pub(crate) use super::util::HashProgress;
use super::util::{PendingImport, import_concurrency, import_pending, walk_input};
use crate::blobs::descriptor::DescriptorHandles;
use crate::fs_plan::SendInput;
use iroh::{
    Endpoint,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
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
/// router — we hand the prepared [`BlobProtocolHandler`] to the caller-provided
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

/// Blob protocol handler with optional provider-side QUIC telemetry.
///
/// The wrapper is used by both the internal Router and the app's shared
/// dispatcher, keeping congestion/loss measurement on the payload-sending
/// endpoint without changing `iroh-blobs` itself.
#[derive(Debug, Clone)]
pub struct BlobProtocolHandler {
    protocol: BlobsProtocol,
    transport_profile: BlobTransportProfile,
    benchmark_run_id: Option<u64>,
}

impl BlobProtocolHandler {
    fn new(
        protocol: BlobsProtocol,
        transport_profile: BlobTransportProfile,
        benchmark_run_id: Option<u64>,
    ) -> Self {
        Self {
            protocol,
            transport_profile,
            benchmark_run_id,
        }
    }
}

impl ProtocolHandler for BlobProtocolHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        // The sampler owns a strong `Connection` clone, and `finish` joins it
        // below, so the connection cannot close before the terminal sample has
        // captured the provider's last congestion counters. Under iroh 0.97 the
        // sampler held a weak `ConnectionInfo` and this function had to keep a
        // strong handle alive by hand.
        let mut telemetry = telemetry_enabled().then(|| {
            BlobProviderTelemetry::start(
                std::time::Instant::now(),
                connection.clone(),
                self.transport_profile,
                self.benchmark_run_id,
            )
        });
        let result = self.protocol.accept(connection).await;
        if let Some(telemetry) = telemetry.as_mut() {
            let outcome = if result.is_ok() {
                TransferEnd::Complete
            } else {
                TransferEnd::Failed
            };
            telemetry.finish(outcome).await;
        }
        result
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
///   collection served by `protocol`.
/// - `unregister_blob_protocol` is called exactly once after the transfer
///   finishes (success or failure).  It MUST clear the registration so the
///   next send can install its own protocol.
///
/// Trait object–safe because methods return boxed futures.
pub trait ExternalBlobRegistrar: std::fmt::Debug + Send + Sync + 'static {
    fn register_blob_protocol(
        &self,
        protocol: BlobProtocolHandler,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    fn unregister_blob_protocol(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

#[derive(Debug)]
pub(crate) struct PreparedStore {
    store: FsStore,
    collection_tag: TempTag,
    files: Vec<PreparedFile>,
    timings: PrepareTimings,
    /// Descriptors the store reads its sources through.  Declared last so they
    /// outlive the store itself: the store reads a referenced file every time
    /// it serves bytes, not just at import.
    _descriptors: DescriptorHandles,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PrepareTimings {
    pub(crate) walk_metadata: Duration,
    pub(crate) import_hash: Duration,
    pub(crate) collection_store: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedFile {
    pub(crate) path: String,
    pub(crate) size: u64,
}

impl PreparedStore {
    pub(crate) async fn prepare(root_dir: &Path, inputs: Vec<SendInput>) -> Result<Self> {
        Self::prepare_with_progress(root_dir, inputs, None).await
    }

    /// [`prepare`](Self::prepare) with a running count of bytes hashed, for the
    /// caller that shows the user what the wait is.
    pub(crate) async fn prepare_with_progress(
        root_dir: &Path,
        inputs: Vec<SendInput>,
        progress: Option<HashProgress>,
    ) -> Result<Self> {
        let store = FsStore::load(root_dir)
            .await
            .map_err(|source| BlobError::store_load(root_dir.to_path_buf(), source))?;

        let mut collection = Collection::default();
        let mut seen_transfer_paths = HashSet::new();
        let mut files_out = Vec::new();
        let mut timings = PrepareTimings::default();
        let mut descriptors = DescriptorHandles::new();

        // Walk every input first, then hash all of their files under one
        // concurrency window. Walking is `stat` and `read_dir`, so it stays
        // serial and the file order — and the order of any error — is what it
        // always was. Hashing is the expensive half and now overlaps across
        // files; see `import_pending`. One window for the whole send, not one
        // per input, because Android hands over a folder as one descriptor
        // input per file: per-input windows would leave 1911 inputs of one
        // file each running strictly serially, which is the case this exists
        // for.
        let walk_started = Instant::now();
        let mut pending: Vec<PendingImport> = Vec::new();
        for input in inputs {
            let input_display = input.path().display().to_string();
            trace!(input_path = %input_display, "processing import input path");
            pending.extend(walk_input(input, &mut descriptors).map_err(|source| {
                BlobError::import_files(
                    input_display.clone(),
                    BlobTextError::new(format!("{source:#}")),
                )
            })?);
        }
        timings.walk_metadata = walk_started.elapsed();

        let import_started = Instant::now();
        let imported = import_pending(&store, pending, import_concurrency(), progress).await?;
        timings.import_hash = import_started.elapsed();

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

        files_out.sort_by(|left, right| left.path.cmp(&right.path));

        let collection_store_started = Instant::now();
        let collection_tag = collection
            .store(store.as_ref())
            .await
            .map_err(|source| BlobError::store_collection(source))?;
        timings.collection_store = collection_store_started.elapsed();
        trace!(
            collection_hash = %collection_tag.hash(),
            item_count = seen_transfer_paths.len(),
            "stored collection in blob store"
        );

        Ok(Self {
            store,
            collection_tag,
            files: files_out,
            timings,
            _descriptors: descriptors,
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

    pub(crate) fn timings(&self) -> PrepareTimings {
        self.timings
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
    transport_profile: BlobTransportProfile,
    benchmark_run_id: Option<u64>,
    /// The receiver this transfer is for. The LAN TCP port serves it and
    /// refuses anyone else; without it there is no LAN port at all, because a
    /// port that would serve any caller is not what this ships.
    expected_peer: Option<iroh::PublicKey>,
}

#[derive(Debug)]
pub(crate) struct BlobRegistration {
    _prepared: PreparedStore,
    inner: BlobRegistrationInner,
    ticket: BlobTicket,
    /// The LAN TCP port serving the same store, when one could be bound.
    /// Dropped with the registration, which closes the port.
    lan_tcp: Option<LanBlobProvider>,
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
    /// We hold `protocol` here only to keep its `Arc<BlobsInner>` alive
    /// until shutdown — the registrar already has its own clone.
    External {
        registrar: Arc<dyn ExternalBlobRegistrar>,
        _protocol: BlobProtocolHandler,
    },
}

impl BlobService {
    pub(crate) fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            transport_profile: BlobTransportProfile::default(),
            benchmark_run_id: None,
            expected_peer: None,
        }
    }

    /// Names the peer allowed to fetch over the LAN TCP transport. Without it
    /// the transport is not offered.
    pub(crate) fn with_expected_peer(mut self, peer: iroh::PublicKey) -> Self {
        self.expected_peer = Some(peer);
        self
    }

    pub(crate) fn with_transport_profile(
        mut self,
        transport_profile: BlobTransportProfile,
    ) -> Self {
        self.transport_profile = transport_profile;
        self
    }

    pub(crate) fn with_session_id(mut self, session_id: &str) -> Self {
        self.benchmark_run_id = benchmark_run_id(session_id);
        self
    }

    /// Register `prepared` for serving using the supplied strategy.  See
    /// [`BlobServingStrategy`] for the trade-offs.
    pub(crate) async fn register_with_strategy(
        self,
        prepared: PreparedStore,
        strategy: &BlobServingStrategy,
    ) -> Result<BlobRegistration> {
        let protocol = BlobProtocolHandler::new(
            BlobsProtocol::new(prepared.store().as_ref(), None),
            self.transport_profile,
            self.benchmark_run_id,
        );
        let ticket = BlobTicket::new(
            self.endpoint.addr(),
            prepared.collection_tag().hash(),
            BlobFormat::HashSeq,
        );

        // Best effort, and only for a known peer: a receiver that never learns
        // a port simply uses QUIC, so failing to bind — or not knowing who to
        // serve — is a lost speedup rather than a failed transfer.
        let lan_tcp = match self.expected_peer {
            None => None,
            Some(expected_peer) => match LanBlobProvider::start(
                prepared.store().as_ref().clone(),
                self.endpoint.secret_key().clone(),
                expected_peer,
            )
            .await
            {
                Ok(provider) => Some(provider),
                Err(error) => {
                    tracing::warn!(
                        target: "wisp_core::blobs::send",
                        %error,
                        "lan tcp provider unavailable; serving over quic only"
                    );
                    None
                }
            },
        };

        let inner = match strategy {
            BlobServingStrategy::Internal => {
                tracing::debug!(
                    target: "wisp_core::blobs::send",
                    "registering blob protocol via Internal Router (sender owns dedicated endpoint)"
                );
                let router = Router::builder(self.endpoint)
                    .accept(ALPN, protocol)
                    .spawn();
                BlobRegistrationInner::InternalRouter(router)
            }
            BlobServingStrategy::External(registrar) => {
                tracing::debug!(
                    target: "wisp_core::blobs::send",
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
            lan_tcp,
        })
    }
}

impl BlobRegistration {
    pub(crate) fn ticket(&self) -> &BlobTicket {
        &self.ticket
    }

    /// Port a peer on this LAN can fetch the same blobs from over TCP, when one
    /// is being served. Goes in the ticket message beside [`Self::ticket`].
    pub(crate) fn lan_tcp_port(&self) -> Option<u16> {
        self.lan_tcp.as_ref().map(LanBlobProvider::port)
    }

    pub(crate) async fn shutdown(self) -> Result<()> {
        match self.inner {
            BlobRegistrationInner::InternalRouter(router) => {
                router
                    .shutdown()
                    .await
                    .map_err(|source| BlobError::store_shutdown("blob registration", source))?;
            }
            BlobRegistrationInner::External {
                registrar,
                _protocol,
            } => {
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
    use crate::fs_plan::SendInput;

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
        let root = unique_temp_dir("wisp-one-shot-duplicate-paths");
        let source = root.join("source");
        let store_root = root.join("store");
        std::fs::create_dir_all(&source)?;
        std::fs::create_dir_all(&store_root)?;
        std::fs::write(source.join("same.txt"), b"same")?;

        let err = PreparedStore::prepare(
            &store_root,
            vec![SendInput::from(source.clone()), SendInput::from(source)],
        )
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

        use super::{BlobProtocolHandler, BlobService, BlobServingStrategy, ExternalBlobRegistrar};
        use crate::blobs::error::Result as BlobResult;
        use iroh::{Endpoint, SecretKey};

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
            stored_protocol: Mutex<Option<BlobProtocolHandler>>,
        }

        impl ExternalBlobRegistrar for RecordingRegistrar {
            fn register_blob_protocol(
                &self,
                protocol: BlobProtocolHandler,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = BlobResult<()>> + Send + '_>>
            {
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
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
                Box::pin(async move {
                    self.call_log.lock().unwrap().push("unregister");
                    self.unregister_count
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    *self.stored_protocol.lock().unwrap() = None;
                })
            }
        }

        let root = unique_temp_dir("wisp-blob-external-strategy");
        let source = root.join("source");
        let store_root = root.join("store");
        std::fs::create_dir_all(&source)?;
        std::fs::create_dir_all(&store_root)?;
        std::fs::write(source.join("hello.txt"), b"world")?;
        let prepared = PreparedStore::prepare(&store_root, vec![SendInput::from(source)]).await?;

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
        let strategy =
            BlobServingStrategy::External(Arc::clone(&registrar) as Arc<dyn ExternalBlobRegistrar>);
        let service = BlobService::new(endpoint.clone());
        let registration = service.register_with_strategy(prepared, &strategy).await?;

        assert_eq!(
            registrar
                .register_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "register_blob_protocol must be called exactly once"
        );
        assert_eq!(
            registrar
                .unregister_count
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "unregister_blob_protocol must not be called during registration"
        );
        assert!(
            registrar.stored_protocol.lock().unwrap().is_some(),
            "registrar must have received the BlobsProtocol instance"
        );

        registration.shutdown().await?;

        assert_eq!(
            registrar
                .unregister_count
                .load(std::sync::atomic::Ordering::SeqCst),
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
        let root = unique_temp_dir("wisp-one-shot-symlink-entry");
        let source = root.join("source");
        let store_root = root.join("store");
        std::fs::create_dir_all(&source)?;
        std::fs::create_dir_all(&store_root)?;
        std::fs::write(source.join("real.txt"), b"real")?;
        symlink("real.txt", source.join("link.txt"))?;

        let err = PreparedStore::prepare(&store_root, vec![SendInput::from(source)])
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
