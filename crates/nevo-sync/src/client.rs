//! The client side: signed requests to one paired peer.
//!
//! Uses hyper's small HTTP/1 connection API directly: hyper is already in the build through
//! axum, the client needs no TLS (there is no CA on a LAN; signatures do that job), and
//! connecting ourselves makes the local-address check (D-005) exact: we connect only to the
//! address we checked.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper::header::{HeaderMap, CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use nevo_format::{Keypair, PublicKey};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::auth::{hex, unhex, CHALLENGE_LEN};
use crate::error::{Result, SyncError};
use crate::limits::MAX_RESPONSE_BYTES;
use crate::msg::{
    BlobRequest, BlobResponse, EntriesRequest, EntriesResponse, HeadsRequest, HeadsResponse, Hello, Message,
    ObjectsRequest, ObjectsResponse,
};
use crate::{headers, is_local, sign_request, verify_response, PROTOCOL_VERSION};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// One request, including a 4 MiB blob part over a slow rooftop link (1 Mbit/s needs 34 s).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Answers "busy" (503) or "slow down" (429) are retried this many times, a second apart.
const RETRIES: u32 = 5;
const RETRY_WAIT: Duration = Duration::from_secs(1);

/// A connection to one paired peer. Requests go one at a time.
pub struct PeerClient {
    addr: SocketAddr,
    authority: String,
    base: String,
    peer: PublicKey,
    me: Arc<Keypair>,
    session: Mutex<Option<Session>>,
}

/// The server's challenge and the last counter used under it.
struct Session {
    challenge: Vec<u8>,
    count: u64,
}

impl PeerClient {
    /// Checks that `url` (such as `http://192.168.1.20:8080/sync`) is on the local network and
    /// that the node answering there has key `peer`.
    pub async fn connect(url: &str, peer: PublicKey, me: Arc<Keypair>) -> Result<PeerClient> {
        let uri: Uri = url.parse().map_err(|_| SyncError::BadUrl(url.to_string()))?;
        if uri.scheme_str() != Some("http") {
            return Err(SyncError::BadUrl(format!(
                "{url}: use http://; requests are signed rather than encrypted"
            )));
        }
        let authority = uri
            .authority()
            .ok_or_else(|| SyncError::BadUrl(format!("{url}: no host")))?;
        let host = authority.host().trim_start_matches('[').trim_end_matches(']');
        let port = authority.port_u16().unwrap_or(80);
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| SyncError::BadUrl(format!("can't find {host}: {e}")))?
            .collect();
        // Every address must be local: a name that also points at the internet is refused.
        if let Some(outside) = addrs.iter().find(|a| !is_local(a.ip())) {
            return Err(SyncError::NotLocal(outside.to_string()));
        }
        let addr = *addrs
            .first()
            .ok_or_else(|| SyncError::BadUrl(format!("{host} has no address")))?;
        let base = match uri.path().trim_end_matches('/') {
            "" => "/sync".to_string(),
            path => path.to_string(),
        };
        let client = PeerClient {
            addr,
            authority: authority.to_string(),
            base,
            peer,
            me,
            session: Mutex::new(None),
        };
        let session = client.hello().await?;
        *client.lock_session() = Some(session);
        Ok(client)
    }

    /// The peer's key, as paired.
    pub fn peer(&self) -> PublicKey {
        self.peer
    }

    /// This node's key, which signs the requests.
    pub fn me(&self) -> PublicKey {
        self.me.public()
    }

    pub async fn heads(&self, req: &HeadsRequest) -> Result<HeadsResponse> {
        self.call("heads", req).await
    }

    pub async fn entries(&self, req: &EntriesRequest) -> Result<EntriesResponse> {
        self.call("entries", req).await
    }

    pub async fn objects(&self, req: &ObjectsRequest) -> Result<ObjectsResponse> {
        self.call("objects", req).await
    }

    pub async fn blob(&self, req: &BlobRequest) -> Result<BlobResponse> {
        self.call("blob", req).await
    }

    fn lock_session(&self) -> std::sync::MutexGuard<'_, Option<Session>> {
        self.session.lock().unwrap_or_else(|p| p.into_inner())
    }

    async fn hello(&self) -> Result<Session> {
        let (status, _, body) = self.send(Method::GET, "hello", Vec::new(), Vec::new()).await?;
        if status != StatusCode::OK {
            return Err(status_error(status, &body));
        }
        let hello = Hello::decode(&body)?;
        if !hello.versions.contains(&PROTOCOL_VERSION) {
            return Err(SyncError::Version(hello.versions));
        }
        if hello.node != self.peer {
            return Err(SyncError::WrongPeer(format!(
                "expected {}, but {} answered",
                self.peer, hello.node
            )));
        }
        if hello.challenge.len() < CHALLENGE_LEN {
            return Err(SyncError::Protocol("challenge too short".into()));
        }
        Ok(Session {
            challenge: hello.challenge,
            count: 0,
        })
    }

    /// The challenge and next counter value, getting a fresh challenge when there is none.
    async fn next_count(&self) -> Result<(Vec<u8>, u64)> {
        let current = self.lock_session().as_mut().map(|s| {
            s.count += 1;
            (s.challenge.clone(), s.count)
        });
        if let Some(c) = current {
            return Ok(c);
        }
        let session = self.hello().await?;
        let challenge = session.challenge.clone();
        *self.lock_session() = Some(Session { count: 1, ..session });
        Ok((challenge, 1))
    }

    async fn call<Q: Message, A: Message>(&self, op: &str, req: &Q) -> Result<A> {
        let body = req.encode();
        let (mut renewed, mut retries) = (false, 0);
        loop {
            let (challenge, count) = self.next_count().await?;
            let sig = sign_request(&self.me, &challenge, count, op, &body)?;
            let auth = vec![
                (headers::NODE, self.me.public().to_string()),
                (headers::CHALLENGE, hex(&challenge)),
                (headers::COUNT, count.to_string()),
                (headers::SIG, hex(&sig)),
            ];
            let (status, answer_headers, answer) =
                self.send(Method::POST, &format!("v1/{op}"), auth, body.clone()).await?;
            match status {
                StatusCode::OK => {
                    let answer_sig = answer_headers
                        .get(headers::SIG)
                        .and_then(|v| v.to_str().ok())
                        .and_then(unhex)
                        .ok_or_else(|| SyncError::WrongPeer("the answer is not signed".into()))?;
                    verify_response(self.peer, &sig, &answer, &answer_sig)
                        .map_err(|_| SyncError::WrongPeer("the answer is not signed by the paired key".into()))?;
                    return A::decode(&answer);
                }
                // The challenge expired or the server restarted: get a new one, once.
                StatusCode::UNAUTHORIZED if !renewed => {
                    renewed = true;
                    *self.lock_session() = None;
                }
                StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE if retries < RETRIES => {
                    retries += 1;
                    tokio::time::sleep(RETRY_WAIT).await;
                }
                _ => return Err(status_error(status, &answer)),
            }
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        headers: Vec<(&str, String)>,
        body: Vec<u8>,
    ) -> Result<(StatusCode, HeaderMap, Bytes)> {
        let transport = |e: &dyn std::fmt::Display| SyncError::Transport(format!("{}: {e}", self.addr));
        let exchange = async {
            let stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(self.addr))
                .await
                .map_err(|_| transport(&"connecting timed out"))?
                .map_err(|e| transport(&e))?;
            let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
                .await
                .map_err(|e| transport(&e))?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            let mut builder = Request::builder()
                .method(method)
                .uri(format!("{}/{path}", self.base))
                .header(HOST, &self.authority)
                .header(CONTENT_TYPE, "application/cbor");
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            let request = builder
                .body(Full::new(Bytes::from(body)))
                .map_err(|e| SyncError::BadUrl(e.to_string()))?;
            let response = sender.send_request(request).await.map_err(|e| transport(&e))?;
            let (parts, body) = response.into_parts();
            let body = Limited::new(body, MAX_RESPONSE_BYTES)
                .collect()
                .await
                .map_err(|e| transport(&e))?
                .to_bytes();
            Ok((parts.status, parts.headers, body))
        };
        timeout(REQUEST_TIMEOUT, exchange)
            .await
            .map_err(|_| transport(&"request timed out"))?
    }
}

fn status_error(status: StatusCode, body: &[u8]) -> SyncError {
    SyncError::Status {
        status: status.as_u16(),
        message: String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned(),
    }
}
