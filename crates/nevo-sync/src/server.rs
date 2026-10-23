//! The sync server: the routes a node mounts under `/sync`.
//!
//! ```text
//! GET  /hello        who this node is, the protocol versions it speaks, and a fresh challenge
//! POST /v1/heads     the highest signed head of every log this node can serve
//! POST /v1/entries   entries of one log, ending at a signed head where possible
//! POST /v1/objects   objects by ID, only those that may leave this node
//! POST /v1/blob      part of a blob, only blobs of shared revisions
//! ```
//!
//! Every `/v1/` request must be signed by a paired peer (see the `auth` module), and every
//! answer is signed by this node.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use nevo_format::cbor::{self, Value};
use nevo_format::{Id, Keypair, PublicKey, Visibility};
use nevo_store::{Store, StoreError};
use tokio::sync::Semaphore;

use crate::auth::{hex, sign_response, unhex, verify_request, Challenges};
use crate::limits::{MAX_ENTRIES, MAX_HEADS, MAX_IDS, MAX_OBJECT_BYTES, MAX_PIECES, MAX_REQUEST_BYTES, PIECE};
use crate::msg::{
    BlobRequest, BlobResponse, EntriesRequest, EntriesResponse, HeadsRequest, HeadsResponse, Hello, Message,
    ObjectsRequest, ObjectsResponse,
};
use crate::{headers, is_local, SharedPeers, PROTOCOL_VERSION};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Sync requests answered at the same time; more are told to come back later (503). Keeps
    /// a Pi responsive for the people reading on it.
    pub max_concurrent: usize,
    /// Requests per second each peer may make on average (429 beyond), and the burst allowed.
    pub requests_per_sec: f64,
    pub burst: f64,
}

impl Default for ServerConfig {
    fn default() -> ServerConfig {
        ServerConfig {
            max_concurrent: 4,
            requests_per_sec: 20.0,
            burst: 100.0,
        }
    }
}

/// What the sync routes need. Cheap to clone.
#[derive(Clone)]
pub struct SyncServer {
    inner: Arc<Inner>,
}

struct Inner {
    store: Arc<Store>,
    node: Arc<Keypair>,
    peers: SharedPeers,
    config: ServerConfig,
    challenges: Mutex<Challenges>,
    buckets: Mutex<HashMap<PublicKey, Bucket>>,
    busy: Arc<Semaphore>,
}

/// A token bucket: each request takes a token; tokens refill at a steady rate.
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl SyncServer {
    pub fn new(store: Arc<Store>, node: Arc<Keypair>, peers: SharedPeers) -> SyncServer {
        SyncServer::with_config(store, node, peers, ServerConfig::default())
    }

    pub fn with_config(store: Arc<Store>, node: Arc<Keypair>, peers: SharedPeers, config: ServerConfig) -> SyncServer {
        SyncServer {
            inner: Arc::new(Inner {
                store,
                node,
                peers,
                busy: Arc::new(Semaphore::new(config.max_concurrent.max(1))),
                config,
                challenges: Mutex::new(Challenges::default()),
                buckets: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The paired peers. Admin pages change this list; the change applies to the next request.
    pub fn peers(&self) -> &SharedPeers {
        &self.inner.peers
    }
}

/// The sync routes, to mount with `Router::nest("/sync", nevo_sync::router(server))`.
///
/// For the server to refuse internet addresses itself (D-005), serve the app with
/// `into_make_service_with_connect_info::<SocketAddr>()`; without it the check is skipped.
pub fn router<S>(server: SyncServer) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/hello", get(hello))
        .route("/v1/{op}", post(operation))
        .with_state(server)
}

// ---------------------------------------------------------------------------------------------
// Refusals

/// An error answer. Bodies are short plain text for people reading logs.
struct Refusal(StatusCode, String);

fn refuse(status: StatusCode, message: &str) -> Refusal {
    Refusal(status, message.to_string())
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

impl From<StoreError> for Refusal {
    fn from(e: StoreError) -> Refusal {
        Refusal(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

impl From<crate::SyncError> for Refusal {
    fn from(e: crate::SyncError) -> Refusal {
        Refusal(StatusCode::BAD_REQUEST, e.to_string())
    }
}

impl From<std::io::Error> for Refusal {
    fn from(e: std::io::Error) -> Refusal {
        Refusal(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

/// The same answer for "doesn't exist" and "may not leave this node", so a peer can't learn
/// that private content exists.
fn not_available() -> Refusal {
    refuse(StatusCode::NOT_FOUND, "not available")
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // The guarded data stays consistent between statements, so a poisoned lock is safe to use.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------------------------------------
// Handlers

async fn hello(State(server): State<SyncServer>, req: Request) -> Response {
    if let Err(r) = check_address(&req) {
        return r.into_response();
    }
    let challenge = match lock(&server.inner.challenges).issue() {
        Ok(c) => c,
        Err(e) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()).into_response(),
    };
    let hello = Hello {
        versions: vec![PROTOCOL_VERSION],
        node: server.inner.node.public(),
        challenge,
    };
    cbor_response(hello.encode(), None)
}

async fn operation(State(server): State<SyncServer>, Path(op): Path<String>, req: Request) -> Response {
    match server.handle(&op, req).await {
        Ok(r) => r,
        Err(r) => r.into_response(),
    }
}

fn cbor_response(body: Vec<u8>, sig: Option<Vec<u8>>) -> Response {
    let mut res = (StatusCode::OK, [(header::CONTENT_TYPE, "application/cbor")], body).into_response();
    if let Some(sig) = sig {
        if let Ok(value) = hex(&sig).parse() {
            res.headers_mut().insert(headers::SIG, value);
        }
    }
    res
}

/// Refuses connections from outside the local network, when the address is known.
fn check_address(req: &Request) -> Result<(), Refusal> {
    match req.extensions().get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(addr)) if !is_local(addr.ip()) => {
            Err(refuse(StatusCode::FORBIDDEN, "sync is for the local network only"))
        }
        _ => Ok(()),
    }
}

/// The signature headers of a request.
struct Auth {
    node: PublicKey,
    challenge: Vec<u8>,
    count: u64,
    sig: Vec<u8>,
}

impl Auth {
    fn from_headers(h: &HeaderMap) -> Option<Auth> {
        let get = |name: &str| h.get(name).and_then(|v| v.to_str().ok());
        Some(Auth {
            node: get(headers::NODE)?.parse().ok()?,
            challenge: unhex(get(headers::CHALLENGE)?)?,
            count: get(headers::COUNT)?.parse().ok()?,
            sig: unhex(get(headers::SIG)?)?,
        })
    }
}

impl SyncServer {
    async fn handle(&self, op: &str, req: Request) -> Result<Response, Refusal> {
        if !matches!(op, "heads" | "entries" | "objects" | "blob") {
            return Err(refuse(StatusCode::NOT_FOUND, "unknown operation"));
        }
        check_address(&req)?;
        let unauthorized = |why: &str| refuse(StatusCode::UNAUTHORIZED, why);
        let auth = Auth::from_headers(req.headers()).ok_or_else(|| unauthorized("missing or malformed signature"))?;
        let inner = &self.inner;
        let paired = inner
            .peers
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains(auth.node);
        if !paired {
            return Err(refuse(StatusCode::FORBIDDEN, "this node is not paired with yours"));
        }
        let body = to_bytes(req.into_body(), MAX_REQUEST_BYTES)
            .await
            .map_err(|_| refuse(StatusCode::PAYLOAD_TOO_LARGE, "request too large"))?;
        verify_request(auth.node, &auth.challenge, auth.count, op, &body, &auth.sig)
            .map_err(|_| unauthorized("bad signature"))?;
        // Only signed requests reach here, so strangers can't use up a peer's counter or tokens.
        if !lock(&inner.challenges).accept(&auth.challenge, auth.node, auth.count) {
            return Err(unauthorized("unknown, expired or reused challenge; ask /hello again"));
        }
        if !self.take_token(auth.node) {
            return Err(refuse(StatusCode::TOO_MANY_REQUESTS, "too many requests; slow down"));
        }
        let _permit = inner
            .busy
            .clone()
            .try_acquire_owned()
            .map_err(|_| refuse(StatusCode::SERVICE_UNAVAILABLE, "busy; try again soon"))?;

        // Store calls block (SQLite, blob files), so they run off the async threads.
        let server = self.clone();
        let op = op.to_string();
        let answer = tokio::task::spawn_blocking(move || server.answer(&op, &body))
            .await
            .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR, "request failed"))??;
        let sig = sign_response(&inner.node, &auth.sig, &answer)?;
        Ok(cbor_response(answer, Some(sig)))
    }

    fn take_token(&self, peer: PublicKey) -> bool {
        let config = &self.inner.config;
        let now = Instant::now();
        let mut buckets = lock(&self.inner.buckets);
        let bucket = buckets.entry(peer).or_insert(Bucket {
            tokens: config.burst,
            last: now,
        });
        let refill = now.duration_since(bucket.last).as_secs_f64() * config.requests_per_sec;
        bucket.tokens = (bucket.tokens + refill).min(config.burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn answer(&self, op: &str, body: &[u8]) -> Result<Vec<u8>, Refusal> {
        Ok(match op {
            "heads" => self.heads(HeadsRequest::decode(body)?)?.encode(),
            "entries" => self.entries(EntriesRequest::decode(body)?)?.encode(),
            "objects" => self.objects(ObjectsRequest::decode(body)?)?.encode(),
            "blob" => self.blob(BlobRequest::decode(body)?)?.encode(),
            _ => return Err(refuse(StatusCode::NOT_FOUND, "unknown operation")),
        })
    }

    /// Signs this node's own log up to its end, so peers can take everything published so far.
    fn sign_own_head(&self) -> Result<(), Refusal> {
        let (store, node) = (&self.inner.store, &self.inner.node);
        let me = node.public();
        if store.log_position(me)? > store.log_head(me)?.map_or(0, |(seq, _)| seq) {
            store.sign_head(node)?;
        }
        Ok(())
    }

    fn heads(&self, req: HeadsRequest) -> Result<HeadsResponse, Refusal> {
        let store = &self.inner.store;
        self.sign_own_head()?;
        let limit = req.limit.clamp(1, MAX_HEADS) as usize;
        let mut nodes = store.log_nodes(req.after, limit + 1)?;
        let more = nodes.len() > limit;
        nodes.truncate(limit);
        let mut res = HeadsResponse {
            heads: Vec::new(),
            forks: Vec::new(),
            next: if more { nodes.last().copied() } else { None },
        };
        for node in nodes {
            // Only heads whose entries this node has: a peer can't take entries we lack.
            if let Some(head) = store.signed_head(node, store.log_position(node)?)? {
                res.heads.push(head.bytes().to_vec());
            }
            if store.is_forked(node)? && res.forks.len() < MAX_HEADS as usize {
                res.forks.extend(store.fork_evidence(node)?);
            }
        }
        Ok(res)
    }

    fn entries(&self, req: EntriesRequest) -> Result<EntriesResponse, Refusal> {
        let store = &self.inner.store;
        if req.node == self.inner.node.public() {
            self.sign_own_head()?;
        }
        let end = req
            .upto
            .min(store.log_position(req.node)?)
            .min(req.after.saturating_add(MAX_ENTRIES));
        if end <= req.after {
            return Ok(EntriesResponse {
                entries: Vec::new(),
                head: None,
            });
        }
        // End at the highest signed head in range, so the peer can store the batch at once.
        let head = store.signed_head(req.node, end)?.filter(|h| h.object.seq > req.after);
        let stop = head.as_ref().map_or(end, |h| h.object.seq);
        let entries = store
            .log_entries(req.node, req.after, (stop - req.after) as usize)?
            .into_iter()
            .map(|(bytes, _)| bytes)
            .collect();
        Ok(EntriesResponse {
            entries,
            head: head.map(|h| h.bytes().to_vec()),
        })
    }

    fn objects(&self, req: ObjectsRequest) -> Result<ObjectsResponse, Refusal> {
        if req.ids.len() > MAX_IDS {
            return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE, "too many IDs"));
        }
        let store = &self.inner.store;
        let mut res = ObjectsResponse {
            objects: Vec::new(),
            done: 0,
        };
        let mut size = 0;
        for &id in &req.ids {
            if size >= MAX_OBJECT_BYTES {
                break;
            }
            // `shareable_object_bytes` never returns unpublished content. Checking visibility
            // again guards against a peer's log that lists one of our private revisions.
            if let Some(bytes) = store.shareable_object_bytes(id)? {
                if is_public(&bytes) {
                    size += bytes.len();
                    res.objects.push(bytes);
                }
            }
            res.done += 1;
        }
        Ok(res)
    }

    fn blob(&self, req: BlobRequest) -> Result<BlobResponse, Refusal> {
        if !req.from.is_multiple_of(PIECE) {
            return Err(refuse(StatusCode::BAD_REQUEST, "blob parts start at a piece boundary"));
        }
        if !self.blob_shareable(req.id)? {
            return Err(not_available());
        }
        let Some(mut file) = self.inner.store.open_blob(req.id)? else {
            return Err(not_available());
        };
        let size = file.metadata()?.len();
        if req.from > size {
            return Err(refuse(StatusCode::BAD_REQUEST, "starts after the end of the blob"));
        }
        let len = req.len.min(MAX_PIECES * PIECE).min(size - req.from);
        file.seek(SeekFrom::Start(req.from))?;
        let mut data = vec![0u8; len as usize];
        file.read_exact(&mut data)?;
        Ok(BlobResponse { size, data })
    }

    /// A blob may leave this node only if a published public revision (one in a log) uses it,
    /// and no node-only or private revision here does. The second rule stops a probe: a peer
    /// that knows a file's hash could otherwise publish a revision naming it, and learn from
    /// our answer whether a family keeps that file privately here.
    fn blob_shareable(&self, blob: Id) -> Result<bool, Refusal> {
        let store = &self.inner.store;
        let users = store.blob_users(blob)?;
        if users.iter().any(|r| r.object.vis != Visibility::Public) {
            return Ok(false);
        }
        for rev in users {
            if store.shareable_object_bytes(rev.id())?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// False for revisions that are node-only or private.
fn is_public(bytes: &[u8]) -> bool {
    match cbor::decode(bytes) {
        Ok(Value::Map(m)) => match m.get("t") {
            Some(Value::Text(t)) if t == "rev" => {
                matches!(m.get("vis"), Some(Value::Text(v)) if v == Visibility::Public.as_str())
            }
            Some(_) => true,
            None => false,
        },
        _ => false,
    }
}
