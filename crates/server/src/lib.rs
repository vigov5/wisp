use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use rand::{Rng, RngCore};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::net::TcpListener;
use tokio::time::sleep;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use wisp_core::pairing::{DiscoveryError, DiscoverySession, DiscoveryState};
use wisp_core::rendezvous::{
    CODE_ALPHABET, CODE_LENGTH, ClaimPeerResponse, PairStatus, PairStatusResponse,
    RegisterPeerRequest, RegisterPeerResponse, validate_code,
};

const CREATE_LIMIT_PER_MINUTE: usize = 10;
const ACCESS_LIMIT_PER_MINUTE: usize = 60;
const DISCOVERY_TTL_SECONDS: i64 = 300;
const CLEANUP_INTERVAL_SECONDS: u64 = 30;
const MAX_TICKET_LENGTH: usize = 4096;

type SharedState = Arc<AppState>;

#[derive(Debug)]
pub struct AppState {
    pairs: Mutex<HashMap<String, DiscoverySession>>,
    create_limiter: Mutex<RateLimiter>,
    access_limiter: Mutex<RateLimiter>,
}

impl AppState {
    fn new() -> Self {
        Self {
            pairs: Mutex::new(HashMap::new()),
            create_limiter: Mutex::new(RateLimiter::default()),
            access_limiter: Mutex::new(RateLimiter::default()),
        }
    }
}

#[derive(Debug, Default)]
struct RateLimiter {
    entries: HashMap<IpAddr, VecDeque<Instant>>,
}

impl RateLimiter {
    fn check(&mut self, ip: IpAddr, limit: usize) -> Result<(), ApiError> {
        let now = Instant::now();
        let window = self.entries.entry(ip).or_default();
        while let Some(front) = window.front() {
            if now.duration_since(*front) >= Duration::from_secs(60) {
                window.pop_front();
            } else {
                break;
            }
        }

        if window.len() >= limit {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate limit exceeded",
            ));
        }

        window.push_back(now);
        Ok(())
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: self.message,
        });
        (self.status, body).into_response()
    }
}

pub fn app(state: SharedState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/pairs", post(register_peer))
        .route("/v1/pairs/{code}/status", get(get_pair_status))
        .route("/v1/pairs/{code}/claim", post(claim_peer))
        // The browser web-receiver registers/polls cross-origin from a static
        // page, so allow any origin. Only the ~10 KB code/ticket handshake hits
        // this server; file bytes never do.
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state)
}

pub async fn serve(listen_addr: SocketAddr) -> Result<()> {
    init_logging();
    let state = Arc::new(AppState::new());
    tokio::spawn(cleanup_task(state.clone()));

    let listener = TcpListener::bind(listen_addr)
        .await
        .with_context(|| format!("binding rendezvous server on {listen_addr}"))?;

    info!(%listen_addr, "rendezvous server listening");

    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("running rendezvous server")
}

async fn healthz() -> &'static str {
    "ok"
}

async fn register_peer(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<RegisterPeerRequest>,
) -> Result<(StatusCode, Json<RegisterPeerResponse>), ApiError> {
    let ip = client_ip(&headers, addr.ip());
    let client = client_label(ip);
    debug!(%client, client_ip = %ip, "resolved client address");
    info!(
        %client,
        ticket_len = request.ticket.len(),
        "register request received"
    );
    state
        .create_limiter
        .lock()
        .map_err(lock_error)?
        .check(ip, CREATE_LIMIT_PER_MINUTE)?;

    validate_discovery_request(&request)?;

    let now = OffsetDateTime::now_utc();
    let expires_at = now + time::Duration::seconds(DISCOVERY_TTL_SECONDS);
    let session = DiscoverySession::new(request.ticket, now, expires_at)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;

    let mut pairs = state.pairs.lock().map_err(lock_error)?;
    purge_discovery_locked(&mut pairs, now);
    let code = unique_code(&pairs);
    pairs.insert(code.clone(), session);
    let pair_count = pairs.len();
    let expires_at_formatted = format_timestamp(expires_at)?;

    info!(
        %client,
        %code,
        expires_at = %expires_at_formatted,
        pair_count,
        "peer registered"
    );

    Ok((
        StatusCode::CREATED,
        Json(RegisterPeerResponse {
            code,
            expires_at: expires_at_formatted,
        }),
    ))
}

async fn claim_peer(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<ClaimPeerResponse>, ApiError> {
    let ip = client_ip(&headers, addr.ip());
    let client = client_label(ip);
    debug!(%client, client_ip = %ip, "resolved client address");
    info!(%client, %code, "claim request received");
    rate_limit_access(&state, ip)?;
    validate_code_api(&code)?;

    let now = OffsetDateTime::now_utc();
    let mut pairs = state.pairs.lock().map_err(lock_error)?;
    purge_discovery_locked(&mut pairs, now);
    let mut session = pairs
        .remove(&code)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "peer not found"))?;

    match session.claim(now) {
        Ok(ticket) => {
            info!(%client, %code, "peer claimed");
            Ok(Json(ClaimPeerResponse { ticket }))
        }
        Err(DiscoveryError::Claimed) => {
            warn!(%client, %code, "claim rejected because peer was already claimed");
            Err(ApiError::new(
                StatusCode::CONFLICT,
                "peer has already been claimed",
            ))
        }
        Err(DiscoveryError::Expired) => {
            warn!(%client, %code, "claim rejected because peer expired");
            Err(ApiError::new(StatusCode::NOT_FOUND, "peer expired"))
        }
        Err(DiscoveryError::EmptyTicket | DiscoveryError::InvalidExpiry) => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid peer state",
        )),
    }
}

async fn get_pair_status(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<PairStatusResponse>, ApiError> {
    let ip = client_ip(&headers, addr.ip());
    let client = client_label(ip);
    debug!(%client, client_ip = %ip, "resolved client address");
    info!(%client, %code, "status request received");
    rate_limit_access(&state, ip)?;
    validate_code_api(&code)?;

    let now = OffsetDateTime::now_utc();
    let mut pairs = state.pairs.lock().map_err(lock_error)?;
    purge_discovery_locked(&mut pairs, now);
    let session = pairs
        .get_mut(&code)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "peer not found"))?;

    if session.state(now) != DiscoveryState::Open {
        warn!(%client, %code, "status lookup found non-open peer");
        return Err(ApiError::new(StatusCode::NOT_FOUND, "peer not found"));
    }

    info!(%client, %code, status = "open", "status request resolved");
    Ok(Json(PairStatusResponse {
        status: PairStatus::Open,
    }))
}

/// The address the request really came from, honouring the reverse proxy.
///
/// In production Caddy terminates TLS and forwards over the compose network, so
/// `ConnectInfo` is Caddy's container address — the same value for every user,
/// which quietly turns the per-IP rate limiter into a single global bucket.
/// Caddy *appends* the address it observed to `X-Forwarded-For`, so the
/// rightmost entry is the one hop we can trust: anything a client injects into
/// the header ends up to the left of it and is ignored. Deployments with no
/// proxy send no header and fall back to the socket address.
///
/// The header is only believed when the request came **from** a proxy we trust.
/// Without that check any client reaching the server directly could set
/// `X-Forwarded-For` to whatever it liked and get a fresh rate-limit bucket per
/// request, which defeats both limiters: `register_peer` would grow the session
/// map without bound, and the access limiter is what makes guessing a 6-character
/// code out of 32^6 infeasible.
///
/// Note this assumes exactly one trusted proxy. If another one is ever put in
/// front (Cloudflare, a load balancer), the rightmost entry becomes *that*
/// proxy and this needs to read its vendor header instead.
fn client_ip(headers: &HeaderMap, socket_ip: IpAddr) -> IpAddr {
    if !is_trusted_proxy(socket_ip) {
        return socket_ip;
    }
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .next_back()
        .and_then(|value| value.rsplit(',').find_map(parse_forwarded_ip))
        .unwrap_or(socket_ip)
}

/// Addresses whose `X-Forwarded-For` is believed.
///
/// Defaults to loopback and the private ranges, which is where a reverse proxy
/// on a container or host network lives; a request arriving from a public
/// address is treated as the client itself and its header ignored. Set
/// `WISP_TRUSTED_PROXIES` to a comma-separated list of addresses to replace
/// that default when the proxy has a public address of its own — setting it
/// narrows trust to exactly that list.
fn is_trusted_proxy(peer: IpAddr) -> bool {
    if let Some(configured) = configured_trusted_proxies() {
        return configured.contains(&peer);
    }
    match peer {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        // `is_unique_local` and `is_unicast_link_local` are still unstable, so
        // match fc00::/7 and fe80::/10 by hand.
        IpAddr::V6(v6) => {
            let [a, b, ..] = v6.octets();
            v6.is_loopback() || (a & 0xfe) == 0xfc || (a == 0xfe && (b & 0xc0) == 0x80)
        }
    }
}

static TRUSTED_PROXIES: OnceLock<Option<Vec<IpAddr>>> = OnceLock::new();

fn configured_trusted_proxies() -> Option<&'static Vec<IpAddr>> {
    TRUSTED_PROXIES
        .get_or_init(|| {
            let raw = std::env::var("WISP_TRUSTED_PROXIES").ok()?;
            let list: Vec<IpAddr> = raw
                .split(',')
                .filter_map(|entry| {
                    let entry = entry.trim();
                    if entry.is_empty() {
                        return None;
                    }
                    match entry.parse::<IpAddr>() {
                        Ok(ip) => Some(ip),
                        Err(_) => {
                            warn!(%entry, "ignoring unparseable WISP_TRUSTED_PROXIES entry");
                            None
                        }
                    }
                })
                .collect();
            // An empty or entirely unparseable value means "trust nothing"
            // rather than silently reverting to the permissive default.
            Some(list)
        })
        .as_ref()
}

/// Per-process key used to pseudonymise client addresses in logs. Random at
/// startup, never persisted, never logged.
static LOG_KEY: OnceLock<[u8; 32]> = OnceLock::new();

/// A short label standing in for a client address in `info` logs.
///
/// The privacy policy states the rendezvous server does not treat your IP as a
/// stable identifier, so ordinary logs carry this instead of the address. It is
/// enough to follow one client through a register → status → claim sequence, or
/// to spot a single address hammering the rate limiter, but the key is random
/// per process, so a label means nothing across a restart and cannot be walked
/// back to an address. Operational digs that genuinely need the address can
/// turn on `debug`, where it is logged once per request.
fn client_label(ip: IpAddr) -> String {
    let key = LOG_KEY.get_or_init(|| {
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        key
    });

    let hash = blake3::keyed_hash(key, ip.to_string().as_bytes());
    hash.to_hex()[..12].to_owned()
}

fn parse_forwarded_ip(entry: &str) -> Option<IpAddr> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }

    if let Ok(ip) = entry.parse::<IpAddr>() {
        return Some(ip);
    }

    // Some proxies append the source port: `203.0.113.7:51234`, `[2001:db8::1]:51234`.
    if let Ok(addr) = entry.parse::<SocketAddr>() {
        return Some(addr.ip());
    }

    // Bracketed IPv6 without a port.
    entry
        .strip_prefix('[')?
        .strip_suffix(']')?
        .parse::<IpAddr>()
        .ok()
}

fn rate_limit_access(state: &SharedState, ip: IpAddr) -> Result<(), ApiError> {
    state
        .access_limiter
        .lock()
        .map_err(lock_error)?
        .check(ip, ACCESS_LIMIT_PER_MINUTE)
}

fn validate_discovery_request(request: &RegisterPeerRequest) -> Result<(), ApiError> {
    if request.ticket.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "ticket must not be empty",
        ));
    }

    if request.ticket.len() > MAX_TICKET_LENGTH {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "ticket is too large",
        ));
    }

    Ok(())
}

fn validate_code_api(code: &str) -> Result<(), ApiError> {
    validate_code(code).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))
}

fn unique_code<T>(entries: &HashMap<String, T>) -> String {
    let mut rng = rand::thread_rng();
    let alphabet = CODE_ALPHABET.as_bytes();
    loop {
        let code: String = (0..CODE_LENGTH)
            .map(|_| {
                let idx = rng.gen_range(0..alphabet.len());
                alphabet[idx] as char
            })
            .collect();
        if !entries.contains_key(&code) {
            return code;
        }
    }
}

fn purge_discovery_locked(pairs: &mut HashMap<String, DiscoverySession>, now: OffsetDateTime) {
    pairs.retain(|_, session| !session.is_removable(now));
}

fn lock_error<T>(_: T) -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "server state is unavailable",
    )
}

fn format_timestamp(timestamp: OffsetDateTime) -> Result<String, ApiError> {
    timestamp
        .format(&Rfc3339)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

async fn cleanup_task(state: SharedState) {
    loop {
        sleep(Duration::from_secs(CLEANUP_INTERVAL_SECONDS)).await;
        let now = OffsetDateTime::now_utc();
        if let Ok(mut pairs) = state.pairs.lock() {
            let before = pairs.len();
            purge_discovery_locked(&mut pairs, now);
            let after = pairs.len();
            if after != before {
                info!(
                    removed = before - after,
                    remaining = after,
                    "expired peers cleaned up"
                );
            }
        }
    }
}

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("wisp_server=info")),
        )
        .with_target(true)
        .compact()
        .try_init();
}

#[cfg(test)]
mod tests {
    use std::str::from_utf8;

    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, header};
    use tower::ServiceExt;

    use super::*;

    fn test_app() -> Router {
        app(Arc::new(AppState::new()))
    }

    fn request(method: Method, uri: &str, body: Body) -> Request<Body> {
        build_request(method, uri, body, None)
    }

    /// Mimics the production path: every request arrives from Caddy's container
    /// address, with the real client appended to `X-Forwarded-For`.
    fn proxied_request(method: Method, uri: &str, body: Body, forwarded: &str) -> Request<Body> {
        build_request(method, uri, body, Some(forwarded))
    }

    fn build_request(
        method: Method,
        uri: &str,
        body: Body,
        forwarded: Option<&str>,
    ) -> Request<Body> {
        build_request_from(method, uri, body, forwarded, [127, 0, 0, 1])
    }

    /// As above but with the socket peer chosen, so a request can arrive
    /// directly from a public client instead of through the proxy.
    fn build_request_from(
        method: Method,
        uri: &str,
        body: Body,
        forwarded: Option<&str>,
        peer: [u8; 4],
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method.clone()).uri(uri);
        if method == Method::POST {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(forwarded) = forwarded {
            builder = builder.header("x-forwarded-for", forwarded);
        }
        let mut request = builder.body(body).expect("request");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((peer, 4000))));
        request
    }

    fn register_body() -> Vec<u8> {
        serde_json::to_vec(&RegisterPeerRequest {
            ticket: "ticket".to_owned(),
        })
        .expect("register body")
    }

    async fn read_json<T: serde::de::DeserializeOwned>(response: Response) -> T {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("json body")
    }

    #[tokio::test]
    async fn register_and_claim_pair_flow_works_once() {
        let app = test_app();
        let body = serde_json::to_vec(&RegisterPeerRequest {
            ticket: "ticket".to_owned(),
        })
        .expect("register body");

        let response = app
            .clone()
            .oneshot(request(Method::POST, "/v1/pairs", Body::from(body)))
            .await
            .expect("register response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let created: RegisterPeerResponse = read_json(response).await;

        let claimed = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v1/pairs/{}/claim", created.code),
                Body::empty(),
            ))
            .await
            .expect("claim response");
        assert_eq!(claimed.status(), StatusCode::OK);
        let claimed: ClaimPeerResponse = read_json(claimed).await;
        assert_eq!(claimed.ticket, "ticket");

        let second_claim = app
            .oneshot(request(
                Method::POST,
                &format!("/v1/pairs/{}/claim", created.code),
                Body::empty(),
            ))
            .await
            .expect("second claim response");
        assert_eq!(second_claim.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pair_status_is_open_until_claimed() {
        let app = test_app();
        let body = serde_json::to_vec(&RegisterPeerRequest {
            ticket: "ticket".to_owned(),
        })
        .expect("register body");

        let response = app
            .clone()
            .oneshot(request(Method::POST, "/v1/pairs", Body::from(body)))
            .await
            .expect("register response");
        let created: RegisterPeerResponse = read_json(response).await;

        let status = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("/v1/pairs/{}/status", created.code),
                Body::empty(),
            ))
            .await
            .expect("status response");
        assert_eq!(status.status(), StatusCode::OK);
        let status: PairStatusResponse = read_json(status).await;
        assert_eq!(status.status, PairStatus::Open);

        let claimed = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v1/pairs/{}/claim", created.code),
                Body::empty(),
            ))
            .await
            .expect("claim response");
        assert_eq!(claimed.status(), StatusCode::OK);

        let status_after_claim = app
            .oneshot(request(
                Method::GET,
                &format!("/v1/pairs/{}/status", created.code),
                Body::empty(),
            ))
            .await
            .expect("status response");
        assert_eq!(status_after_claim.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_is_rate_limited() {
        let app = test_app();
        for _ in 0..CREATE_LIMIT_PER_MINUTE {
            let body = serde_json::to_vec(&RegisterPeerRequest {
                ticket: "ticket".to_owned(),
            })
            .expect("create body");

            let response = app
                .clone()
                .oneshot(request(Method::POST, "/v1/pairs", Body::from(body)))
                .await
                .expect("create response");
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let body = serde_json::to_vec(&RegisterPeerRequest {
            ticket: "ticket".to_owned(),
        })
        .expect("create body");
        let response = app
            .oneshot(request(Method::POST, "/v1/pairs", Body::from(body)))
            .await
            .expect("rate limit response");
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let body = from_utf8(&bytes).expect("utf8");
        assert!(body.contains("rate limit exceeded"));
    }

    #[tokio::test]
    async fn rate_limit_is_per_forwarded_client_not_per_proxy() {
        let app = test_app();

        // One client behind the proxy burns its whole allowance.
        for _ in 0..CREATE_LIMIT_PER_MINUTE {
            let response = app
                .clone()
                .oneshot(proxied_request(
                    Method::POST,
                    "/v1/pairs",
                    Body::from(register_body()),
                    "203.0.113.7",
                ))
                .await
                .expect("create response");
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let exhausted = app
            .clone()
            .oneshot(proxied_request(
                Method::POST,
                "/v1/pairs",
                Body::from(register_body()),
                "203.0.113.7",
            ))
            .await
            .expect("rate limit response");
        assert_eq!(exhausted.status(), StatusCode::TOO_MANY_REQUESTS);

        // A different client behind the same proxy must be unaffected.
        let other = app
            .oneshot(proxied_request(
                Method::POST,
                "/v1/pairs",
                Body::from(register_body()),
                "198.51.100.4",
            ))
            .await
            .expect("other client response");
        assert_eq!(other.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn forwarded_for_injected_by_the_client_is_ignored() {
        let app = test_app();

        // Caddy appends the address it observed, so the spoofed value sits to
        // the left and the real client is still the one being limited.
        for _ in 0..CREATE_LIMIT_PER_MINUTE {
            let response = app
                .clone()
                .oneshot(proxied_request(
                    Method::POST,
                    "/v1/pairs",
                    Body::from(register_body()),
                    "10.0.0.1, 203.0.113.7",
                ))
                .await
                .expect("create response");
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let rotated_spoof = app
            .oneshot(proxied_request(
                Method::POST,
                "/v1/pairs",
                Body::from(register_body()),
                "10.0.0.2, 203.0.113.7",
            ))
            .await
            .expect("rate limit response");
        assert_eq!(rotated_spoof.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    /// V13. `X-Forwarded-For` is only meaningful from a hop that sets it. A
    /// client reaching the server directly must not be able to choose its own
    /// rate-limit bucket, or `register_peer` grows the session map without
    /// bound and the throttle protecting a 32^6 code space disappears.
    #[tokio::test]
    async fn rate_limit_cannot_be_bypassed_by_spoofing_forwarded_for() {
        let app = test_app();

        // A public client, inventing a different address on every request.
        for attempt in 0..CREATE_LIMIT_PER_MINUTE {
            let response = app
                .clone()
                .oneshot(build_request_from(
                    Method::POST,
                    "/v1/pairs",
                    Body::from(register_body()),
                    Some(&format!("10.0.0.{attempt}")),
                    [203, 0, 113, 9],
                ))
                .await
                .expect("create response");
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let rotated = app
            .oneshot(build_request_from(
                Method::POST,
                "/v1/pairs",
                Body::from(register_body()),
                Some("10.0.0.250"),
                [203, 0, 113, 9],
            ))
            .await
            .expect("rate limit response");
        assert_eq!(
            rotated.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "a direct client rotating X-Forwarded-For got a fresh bucket"
        );
    }

    #[test]
    fn forwarded_for_is_ignored_from_an_untrusted_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.7".parse().expect("header"));

        // Straight from the internet: the header is the client's own claim.
        let public = IpAddr::from([198, 51, 100, 22]);
        assert_eq!(client_ip(&headers, public), public);

        // Through the proxy on a private network: believed.
        let proxy = IpAddr::from([172, 18, 0, 2]);
        assert_eq!(
            client_ip(&headers, proxy),
            "203.0.113.7".parse::<IpAddr>().expect("ip")
        );
    }

    #[test]
    fn trusted_proxy_defaults_cover_the_container_network_only() {
        for trusted in ["127.0.0.1", "10.1.2.3", "172.18.0.2", "192.168.1.5", "::1"] {
            assert!(
                is_trusted_proxy(trusted.parse().expect("ip")),
                "{trusted} should be trusted"
            );
        }
        for untrusted in ["203.0.113.7", "8.8.8.8", "2001:db8::1"] {
            assert!(
                !is_trusted_proxy(untrusted.parse().expect("ip")),
                "{untrusted} should not be trusted"
            );
        }
    }

    #[test]
    fn client_label_hides_the_address_but_stays_stable() {
        let ip: IpAddr = "203.0.113.7".parse().expect("ip");
        let label = client_label(ip);

        assert_eq!(label, client_label(ip));
        assert_ne!(label, client_label("198.51.100.4".parse().expect("ip")));
        assert!(!label.contains("203"), "label leaks the address: {label}");
    }

    #[test]
    fn client_ip_falls_back_to_the_socket_without_a_proxy() {
        let socket = IpAddr::from([127, 0, 0, 1]);
        assert_eq!(client_ip(&HeaderMap::new(), socket), socket);

        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().expect("header"));
        assert_eq!(client_ip(&headers, socket), socket);
    }

    #[test]
    fn client_ip_parses_forwarded_entry_shapes() {
        let socket = IpAddr::from([127, 0, 0, 1]);

        for (forwarded, expected) in [
            ("203.0.113.7", "203.0.113.7"),
            (" 10.0.0.1 ,  203.0.113.7 ", "203.0.113.7"),
            ("203.0.113.7:51234", "203.0.113.7"),
            ("2001:db8::1", "2001:db8::1"),
            ("[2001:db8::1]", "2001:db8::1"),
            ("[2001:db8::1]:51234", "2001:db8::1"),
            // Trailing garbage: fall back to the rightmost parseable entry.
            ("203.0.113.7, unknown", "203.0.113.7"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-for", forwarded.parse().expect("header"));
            assert_eq!(
                client_ip(&headers, socket),
                expected.parse::<IpAddr>().expect("expected ip"),
                "forwarded: {forwarded}"
            );
        }
    }

    #[tokio::test]
    async fn invalid_pair_registration_is_rejected() {
        let app = test_app();
        let body = serde_json::to_vec(&RegisterPeerRequest {
            ticket: String::new(),
        })
        .expect("register body");

        let response = app
            .oneshot(request(Method::POST, "/v1/pairs", Body::from(body)))
            .await
            .expect("invalid response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
