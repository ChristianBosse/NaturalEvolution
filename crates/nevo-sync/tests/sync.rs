//! Real stores in temporary folders, real HTTP servers on 127.0.0.1.

use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use nevo_format::cbor::{self, Map, Value};
use nevo_format::*;
use nevo_store::{Store, StoreError};
use nevo_sync::limits::PIECE;
use nevo_sync::msg::*;
use nevo_sync::*;

struct Node {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    key: Arc<Keypair>,
    peers: SharedPeers,
    url: String,
}

async fn node(seed: u8) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().join("library")).unwrap());
    let key = Arc::new(Keypair::from_secret([seed; 32]));
    let peers = PeerList::new().shared();
    let url = serve(Router::new().nest(
        "/sync",
        router(SyncServer::new(store.clone(), key.clone(), peers.clone())),
    ))
    .await;
    Node {
        _dir: dir,
        store,
        key,
        peers,
        url,
    }
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap()
    });
    format!("http://{addr}/sync")
}

/// The admins of both nodes approve each other.
fn pair(a: &Node, b: &Node) {
    for (x, y) in [(a, b), (b, a)] {
        x.peers.write().unwrap().add(Peer {
            key: y.key.public(),
            address: Some(y.url.clone()),
            name: "peer".into(),
        });
    }
}

async fn connect(to: &Node, from: &Node) -> PeerClient {
    PeerClient::connect(&from.url, from.key.public(), to.key.clone())
        .await
        .unwrap()
}

async fn pull_from(to: &Node, from: &Node) -> PullReport {
    let peer = connect(to, from).await;
    pull(&to.store, &peer, &PullOptions::new(&to.store)).await.unwrap()
}

fn guide(node: &Node, title: &str, content: &[u8], vis: Visibility) -> Sealed<Revision> {
    let blob = node.store.put_blob(content).unwrap();
    let revision = Revision {
        item: None,
        parents: vec![],
        author: node.key.public(),
        clock: node.store.author_clock(node.key.public()).unwrap() + 1,
        time: Time {
            ms: 1_792_000_000_000,
            src: TimeSource::Unknown,
        },
        kind: Kind::Guide,
        title: title.into(),
        lang: "en".into(),
        topics: vec!["water/treatment".parse().unwrap()],
        license: "CC-BY-SA-4.0".into(),
        vis,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob: blob.id,
            size: blob.size,
            mime: "text/markdown".into(),
        }],
        source: None,
        summary: None,
        extra: Map::new(),
    };
    let sealed = seal(&revision, &node.key).unwrap();
    node.store.put_object(sealed.bytes()).unwrap();
    sealed
}

fn publish(node: &Node, title: &str, content: &[u8]) -> Sealed<Revision> {
    let r = guide(node, title, content, Visibility::Public);
    node.store.publish(&node.key, r.bytes()).unwrap();
    r
}

fn blob_of(r: &Sealed<Revision>) -> Id {
    r.object.content[0].blob
}

/// `n` has every revision in `revs`, each blob intact, and `origin`'s whole log.
fn has_everything(n: &Node, origin: &Node, revs: &[Sealed<Revision>]) {
    for r in revs {
        assert!(n.store.has_object(r.id()).unwrap(), "{}", r.object.title);
        assert_eq!(
            n.store.verify_blob(blob_of(r)).unwrap(),
            Some(true),
            "{}",
            r.object.title
        );
    }
    let node = origin.key.public();
    assert_eq!(
        n.store.log_position(node).unwrap(),
        origin.store.log_position(node).unwrap()
    );
}

/// Bytes that don't compress or repeat, `len` long.
fn big(len: usize, seed: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 32);
    let mut block = blake3::hash(&[seed]);
    while out.len() < len {
        out.extend_from_slice(block.as_bytes());
        block = blake3::hash(block.as_bytes());
    }
    out.truncate(len);
    out
}

// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn knowledge_spreads_by_contact() {
    let (a, b, c) = (node(1).await, node(2).await, node(3).await);
    pair(&a, &b);
    pair(&b, &c);
    let mut revs: Vec<_> = (0..3)
        .map(|i| {
            publish(
                &a,
                &format!("Guide {i}"),
                format!("Boil water for {i} minutes").as_bytes(),
            )
        })
        .collect();

    let r = pull_from(&b, &a).await;
    assert!(r.complete && r.problems.is_empty(), "{r:?}");
    assert_eq!((r.logs, r.entries, r.objects, r.blobs), (1, 3, 3, 3));
    has_everything(&b, &a, &revs);

    // Pulling again changes nothing.
    let again = pull_from(&b, &a).await;
    assert_eq!((again.entries, again.objects, again.blobs), (0, 0, 0), "{again:?}");

    // New work on A arrives on the next pull.
    revs.push(publish(&a, "Guide 3", b"Filter through cloth first"));
    let r = pull_from(&b, &a).await;
    assert_eq!((r.entries, r.objects, r.blobs), (1, 1, 1), "{r:?}");
    has_everything(&b, &a, &revs);

    // C never met A, yet gets A's work through B, still verified against A's signed head.
    let r = pull_from(&c, &b).await;
    assert!(r.complete && r.problems.is_empty(), "{r:?}");
    has_everything(&c, &a, &revs);
    assert_eq!(
        c.store.log_head(a.key.public()).unwrap(),
        a.store.log_head(a.key.public()).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn private_and_node_only_content_never_transfers() {
    let (a, b) = (node(1).await, node(2).await);
    pair(&a, &b);
    let public = publish(&a, "Water", b"public guide");
    let private = guide(&a, "Medical history", b"private record", Visibility::Private);
    let node_only = guide(&a, "Staff rota", b"node only", Visibility::Node);
    assert!(matches!(
        a.store.publish(&a.key, private.bytes()),
        Err(StoreError::NotShareable)
    ));

    // Another node's log reaches A, listing A's private revision and a public revision of its
    // own that names the private file's hash (a probe). Neither may pull private data out.
    let other = Keypair::from_secret([66; 32]);
    let mut probe = public.object.clone();
    probe.author = other.public();
    probe.content[0].blob = blob_of(&private);
    let probe = seal(&probe, &other).unwrap();
    a.store.put_object(probe.bytes()).unwrap();
    let first = seal_unsigned(&LogEntry {
        seq: 1,
        prev: None,
        obj: private.id(),
        extra: Map::new(),
    })
    .unwrap();
    let second = seal_unsigned(&LogEntry {
        seq: 2,
        prev: Some(first.id()),
        obj: probe.id(),
        extra: Map::new(),
    })
    .unwrap();
    let head = seal(
        &LogHead {
            node: other.public(),
            seq: 2,
            entry: second.id(),
            extra: Map::new(),
        },
        &other,
    )
    .unwrap();
    a.store.put_object(head.bytes()).unwrap();
    a.store
        .append_log_entries(other.public(), &[first.bytes().to_vec(), second.bytes().to_vec()])
        .unwrap();

    let r = pull_from(&b, &a).await;
    assert!(b.store.has_object(public.id()).unwrap());
    assert!(b.store.has_blob(blob_of(&public)));
    assert!(b.store.has_object(probe.id()).unwrap(), "public metadata still moves");
    for hidden in [&private, &node_only] {
        assert!(!b.store.has_object(hidden.id()).unwrap(), "{}", hidden.object.title);
        assert!(!b.store.has_blob(blob_of(hidden)), "{}", hidden.object.title);
    }
    assert_eq!(r.objects, 2, "{r:?}");

    // Asking directly gets nothing either, and the same answer as for things that don't exist.
    let peer = connect(&b, &a).await;
    let res = peer
        .objects(&ObjectsRequest {
            ids: vec![private.id(), node_only.id(), public.id()],
        })
        .await
        .unwrap();
    assert_eq!(res.objects, vec![public.bytes().to_vec()]);
    for id in [blob_of(&private), blob_of(&node_only), Id::of(b"no such blob")] {
        let err = peer
            .blob(&BlobRequest {
                id,
                from: 0,
                len: PIECE,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, SyncError::Status { status: 404, .. }), "{err}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unpaired_and_impostor_peers_are_refused() {
    let (a, b, c) = (node(1).await, node(2).await, node(3).await);
    pair(&a, &b);
    publish(&a, "Water", b"guide");

    // C was never approved by A's admin.
    let peer = connect(&c, &a).await;
    let err = pull(&c.store, &peer, &PullOptions::new(&c.store)).await.unwrap_err();
    assert!(matches!(err, SyncError::Status { status: 403, .. }), "{err}");
    assert_eq!(c.store.log_position(a.key.public()).unwrap(), 0);

    // B expects C at A's address: whoever answers there is not the node B paired with.
    let err = PeerClient::connect(&a.url, c.key.public(), b.key.clone())
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SyncError::WrongPeer(_)), "{err}");

    // Unpairing applies to the next request.
    let peer = connect(&b, &a).await;
    a.peers.write().unwrap().remove(b.key.public());
    let err = pull(&b.store, &peer, &PullOptions::new(&b.store)).await.unwrap_err();
    assert!(matches!(err, SyncError::Status { status: 403, .. }), "{err}");

    // Never the internet (D-005), and no TLS pretending to help.
    let err = PeerClient::connect("http://8.8.8.8:8080/sync", a.key.public(), b.key.clone())
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SyncError::NotLocal(_)), "{err}");
    let err = PeerClient::connect("https://127.0.0.1:1/sync", a.key.public(), b.key.clone())
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SyncError::BadUrl(_)), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn forks_are_detected_and_passed_on() {
    // Two stores with the same node key: the node signs two different logs.
    let (a1, a2, b, c) = (node(1).await, node(1).await, node(2).await, node(3).await);
    pair(&a1, &b);
    pair(&a2, &b);
    pair(&b, &c);
    let original = publish(&a1, "Original", b"one log");
    publish(&a2, "Rewritten", b"another log");
    let a = a1.key.public();

    let r = pull_from(&b, &a1).await;
    assert!(r.forks.is_empty() && r.problems.is_empty(), "{r:?}");
    let r = pull_from(&b, &a2).await;
    assert_eq!(r.forks, vec![a], "{r:?}");
    assert!(b.store.is_forked(a).unwrap());

    // C learns about the fork from B, with the signed proof, and keeps the first log.
    let r = pull_from(&c, &b).await;
    assert_eq!(r.forks, vec![a], "{r:?}");
    assert!(c.store.is_forked(a).unwrap());
    assert!(c.store.has_object(original.id()).unwrap());
    assert_eq!(c.store.log_position(a).unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupted_pulls_resume() {
    let (a, b) = (node(1).await, node(2).await);
    pair(&a, &b);
    let mut revs: Vec<_> = (0..5)
        .map(|i| publish(&a, &format!("Guide {i}"), format!("text {i}").as_bytes()))
        .collect();
    revs.push(publish(&a, "Video", &big(5 * PIECE as usize / 2, 1)));

    // Small limits stop each round early; every round keeps what it verified.
    let peer = connect(&b, &a).await;
    let options = PullOptions {
        max_objects: 2,
        max_blob_bytes: PIECE,
        ..PullOptions::new(&b.store)
    };
    let partial = options.partial_dir.join(blob_of(&revs[5]).to_string());
    let (mut rounds, mut saw_partial) = (0, false);
    loop {
        let r = pull(&b.store, &peer, &options).await.unwrap();
        assert!(r.problems.is_empty(), "{r:?}");
        rounds += 1;
        saw_partial |= partial.exists();
        if r.complete {
            break;
        }
        assert!(rounds < 20, "no progress");
    }
    assert!(rounds >= 3 && saw_partial, "{rounds} rounds");
    assert!(!partial.exists());
    has_everything(&b, &a, &revs);

    // A round cut off at any moment (a lost link) leaves a store the next round completes.
    let c = node(3).await;
    pair(&a, &c);
    let peer = connect(&c, &a).await;
    let options = PullOptions::new(&c.store);
    for ms in [1, 5, 20] {
        let _ = tokio::time::timeout(Duration::from_millis(ms), pull(&c.store, &peer, &options)).await;
    }
    let r = pull(&c.store, &peer, &options).await.unwrap();
    assert!(r.complete && r.problems.is_empty(), "{r:?}");
    has_everything(&c, &a, &revs);
}

#[tokio::test(flavor = "multi_thread")]
async fn corrupt_blobs_are_refused() {
    let (a, b) = (node(1).await, node(2).await);
    pair(&a, &b);
    let content = big(2 * PIECE as usize + 10, 2);
    let rev = publish(&a, "Map", &content);
    let blob = blob_of(&rev);

    // A's copy decays on its SD card; A serves it without knowing.
    let path = a.store.blob_path(blob);
    let mut bytes = fs::read(&path).unwrap();
    bytes[PIECE as usize + 7] ^= 1;
    fs::write(&path, bytes).unwrap();

    let r = pull_from(&b, &a).await;
    assert!(b.store.has_object(rev.id()).unwrap(), "metadata still moves");
    assert!(!b.store.has_blob(blob));
    assert_eq!(r.blobs, 0);
    assert!(r.problems.iter().any(|p| p.contains("hashes to")), "{r:?}");
    assert!(!PullOptions::new(&b.store).partial_dir.join(blob.to_string()).exists());

    // Once A repairs its copy, B gets a good one.
    a.store.put_blob(&content[..]).unwrap();
    let r = pull_from(&b, &a).await;
    assert_eq!(r.blobs, 1, "{r:?}");
    assert_eq!(b.store.verify_blob(blob).unwrap(), Some(true));
}

// ---------------------------------------------------------------------------------------------
// A malicious peer: it speaks the protocol and signs its answers, but sends what it likes.

#[derive(Clone)]
struct Evil {
    key: Arc<Keypair>,
    heads: HeadsResponse,
    entries: EntriesResponse,
    objects: ObjectsResponse,
}

async fn evil_hello(State(evil): State<Evil>) -> Vec<u8> {
    Hello {
        versions: vec![1],
        node: evil.key.public(),
        challenge: vec![7; 16],
    }
    .encode()
}

async fn evil_op(State(evil): State<Evil>, Path(op): Path<String>, headers: HeaderMap) -> Response {
    let body = match op.as_str() {
        "heads" => evil.heads.encode(),
        "entries" => evil.entries.encode(),
        "objects" => evil.objects.encode(),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let request_sig = unhex(headers[nevo_sync::headers::SIG].to_str().unwrap());
    let sig = sign_response(&evil.key, &request_sig, &body).unwrap();
    ([(nevo_sync::headers::SIG, hex(&sig))], body).into_response()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

async fn evil_node(seed: u8, heads: Vec<Vec<u8>>, entries: Vec<Vec<u8>>, objects: Vec<Vec<u8>>) -> (String, PublicKey) {
    let evil = Evil {
        key: Arc::new(Keypair::from_secret([seed; 32])),
        heads: HeadsResponse {
            heads,
            forks: vec![],
            next: None,
        },
        entries: EntriesResponse { entries, head: None },
        objects: ObjectsResponse {
            done: objects.len() as u64,
            objects,
        },
    };
    let key = evil.key.public();
    let app = Router::new().nest(
        "/sync",
        Router::new()
            .route("/hello", get(evil_hello))
            .route("/v1/{op}", post(evil_op))
            .with_state(evil),
    );
    (serve(app).await, key)
}

#[tokio::test(flavor = "multi_thread")]
async fn forged_heads_entries_and_objects_are_refused() {
    let (a, b) = (node(1).await, node(2).await);
    let revs = [publish(&a, "One", b"1"), publish(&a, "Two", b"2")];
    let head = a.store.sign_head(&a.key).unwrap().unwrap();
    let entries: Vec<Vec<u8>> = a
        .store
        .log_entries(a.key.public(), 0, 10)
        .unwrap()
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    let pull_evil = |url: String, key: PublicKey| {
        let b_key = b.key.clone();
        let store = b.store.clone();
        async move {
            let peer = PeerClient::connect(&url, key, b_key).await.unwrap();
            pull(&store, &peer, &PullOptions::new(&store)).await.unwrap()
        }
    };

    // 1. A head that claims more than A signed.
    let mut forged = match cbor::decode(head.bytes()).unwrap() {
        Value::Map(m) => m,
        _ => unreachable!(),
    };
    forged.insert("seq", 5u64);
    let (url, key) = evil_node(90, vec![cbor::encode_map(&forged)], vec![], vec![]).await;
    let r = pull_evil(url, key).await;
    assert!(r.problems.iter().any(|p| p.contains("refused a log head")), "{r:?}");
    assert_eq!(b.store.log_position(a.key.public()).unwrap(), 0);

    // 2. A's real head with an entry swapped to list another object.
    let swapped = seal_unsigned(&LogEntry {
        seq: 2,
        prev: Some(Id::of(&entries[0])),
        obj: Id::of(b"injected"),
        extra: Map::new(),
    })
    .unwrap();
    let tampered = vec![entries[0].clone(), swapped.bytes().to_vec()];
    let (url, key) = evil_node(91, vec![head.bytes().to_vec()], tampered, vec![]).await;
    let r = pull_evil(url, key).await;
    assert!(r.problems.iter().any(|p| p.contains("refused entries")), "{r:?}");
    assert_eq!(b.store.log_position(a.key.public()).unwrap(), 0);

    // 3. The honest log, but objects nobody listed and a revision altered after signing.
    let stranger = Keypair::from_secret([92; 32]);
    let mut planted = revs[0].object.clone();
    planted.author = stranger.public();
    let planted = seal(&planted, &stranger).unwrap();
    let mut altered = match cbor::decode(revs[1].bytes()).unwrap() {
        Value::Map(m) => m,
        _ => unreachable!(),
    };
    altered.insert("title", "Two (edited)");
    let objects = vec![
        planted.bytes().to_vec(),
        cbor::encode_map(&altered),
        revs[0].bytes().to_vec(),
    ];
    let (url, key) = evil_node(93, vec![head.bytes().to_vec()], entries.clone(), objects).await;
    let r = pull_evil(url, key).await;
    assert_eq!(b.store.log_position(a.key.public()).unwrap(), 2);
    assert_eq!(r.objects, 1, "{r:?}");
    assert!(b.store.has_object(revs[0].id()).unwrap());
    assert!(!b.store.has_object(planted.id()).unwrap());
    assert!(
        !b.store.has_object(revs[1].id()).unwrap(),
        "the altered copy has another ID"
    );
    assert_eq!(
        r.problems.iter().filter(|p| p.contains("wasn't asked for")).count(),
        2,
        "{r:?}"
    );
}
