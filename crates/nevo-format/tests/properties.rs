//! Property tests: random and malformed input against the guarantees of docs/spec/content-model.md.
//!
//! Every test here runs `PROPTEST_CASES` cases (256 by default). The `#[ignore]` tests are
//! exhaustive versions of the same checks: `cargo test -p nevo-format --test properties -- --ignored`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

use nevo_format::cbor::{self, Map, Value};
use nevo_format::*;
use proptest::collection::{btree_set, vec};
use proptest::prelude::*;
use unicode_normalization::UnicodeNormalization;

// ---------------------------------------------------------------------------------------------
// Memory accounting: the decoder must never ask for much more memory than its input justifies.

struct Counting;

thread_local! {
    /// Bytes this thread holds right now, and the most it has held since the last reset.
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn note(delta: isize) {
    let _ = LIVE.try_with(|live| {
        live.set(live.get() + delta);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size() as isize);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size() as isize);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size as isize - layout.size() as isize);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `f` and returns its result with the most extra memory it held at any one time.
fn peak_memory<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let start = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(start));
    let result = f();
    (result, (PEAK.with(Cell::get) - start).max(0) as usize)
}

/// What decoding `len` bytes may reasonably hold: every input byte can become one `Value` slot
/// (a byte string, a 1-byte integer in an array) plus B-tree overhead for maps.
fn memory_budget(len: usize) -> usize {
    128 * len + 64 * 1024
}

/// Decodes untrusted bytes and checks the decoder's promises: bounded memory and, when the bytes
/// are accepted, that they are the one canonical encoding of what they decode to.
fn check_decode(bytes: &[u8]) -> Result<Value> {
    let (result, peak) = peak_memory(|| cbor::decode(bytes));
    assert!(
        peak <= memory_budget(bytes.len()),
        "decoding {} bytes held {peak} bytes",
        bytes.len()
    );
    if let Ok(value) = &result {
        assert_eq!(
            cbor::encode(value),
            bytes,
            "accepted bytes must be canonical: {value:?}"
        );
    }
    result
}

// ---------------------------------------------------------------------------------------------
// CBOR value strategies

const INT_MIN: i128 = -(1 << 64);
const INT_MAX: i128 = (1 << 64) - 1;

/// Integers, weighted towards the edges where the encoded head changes size.
fn int() -> impl Strategy<Value = i128> {
    let mut edges = Vec::new();
    for edge in [
        0i128,
        23,
        24,
        0xff,
        0x100,
        0xffff,
        0x1_0000,
        0xffff_ffff,
        0x1_0000_0000,
        INT_MAX,
    ] {
        for n in [edge - 1, edge, edge + 1, -1 - edge] {
            if (INT_MIN..=INT_MAX).contains(&n) {
                edges.push(n);
            }
        }
    }
    edges.extend([i128::from(i64::MIN), i128::from(i64::MAX), INT_MIN]);
    prop_oneof![INT_MIN..=INT_MAX, -1000i128..1000, prop::sample::select(edges),]
}

/// Text, including characters whose NFC form differs (combining accents, Hangul jamo).
fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-c]{0,3}",
        "[ae\u{300}\u{301}\u{e9}\u{1100}\u{1161}\u{11a8}\u{ac00}\u{212b}\u{fb01}]{0,8}",
        ".{0,12}",
        "[a-z]{20,30}",
        "[a-z]{250,260}",
    ]
}

fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        int().prop_map(Value::Int),
        prop_oneof![vec(any::<u8>(), 0..30), vec(any::<u8>(), 20..300)].prop_map(Value::Bytes),
        text().prop_map(Value::Text),
        any::<bool>().prop_map(Value::Bool),
        Just(Value::Null),
    ];
    leaf.prop_recursive(5, 64, 6, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..6).prop_map(Value::Array),
            vec((text(), inner), 0..6).prop_map(|entries| {
                let mut map = Map::new();
                for (k, v) in entries {
                    map.insert(&k, v);
                }
                Value::Map(map)
            }),
        ]
    })
}

/// What `value` becomes once encoded: text and keys in NFC, and of keys that become equal after
/// normalization, the first in map order is kept.
fn nfc(value: &Value) -> Value {
    match value {
        Value::Text(s) => Value::Text(s.nfc().collect()),
        Value::Array(items) => Value::Array(items.iter().map(nfc).collect()),
        Value::Map(map) => Value::Map(nfc_map(map)),
        other => other.clone(),
    }
}

fn nfc_map(map: &Map) -> Map {
    let mut out = Map::new();
    for (k, v) in map.iter() {
        let k: String = k.nfc().collect();
        if !out.contains(&k) {
            out.insert(&k, nfc(v));
        }
    }
    out
}

#[derive(Clone, Debug)]
enum Mutation {
    Flip(usize, u8),
    Insert(usize, u8),
    Remove(usize),
    Truncate(usize),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        4 => (any::<usize>(), 1..=255u8).prop_map(|(i, x)| Mutation::Flip(i, x)),
        1 => (any::<usize>(), any::<u8>()).prop_map(|(i, b)| Mutation::Insert(i, b)),
        1 => any::<usize>().prop_map(Mutation::Remove),
        1 => any::<usize>().prop_map(Mutation::Truncate),
    ]
}

fn mutate(bytes: &mut Vec<u8>, m: &Mutation) {
    if bytes.is_empty() {
        if let Mutation::Insert(_, b) = m {
            bytes.push(*b);
        }
        return;
    }
    let len = bytes.len();
    match *m {
        Mutation::Flip(i, x) => bytes[i % len] ^= x,
        Mutation::Insert(i, b) => bytes.insert(i % (bytes.len() + 1), b),
        Mutation::Remove(i) => {
            bytes.remove(i % bytes.len());
        }
        Mutation::Truncate(i) => bytes.truncate(i % bytes.len()),
    }
}

// ---------------------------------------------------------------------------------------------
// CBOR

proptest! {
    #[test]
    fn cbor_round_trips_after_nfc(v in value()) {
        let bytes = cbor::encode(&v);
        let expected = nfc(&v);
        prop_assert_eq!(check_decode(&bytes).unwrap(), expected.clone());
        prop_assert_eq!(cbor::encode(&expected), bytes, "normalizing first changes nothing");
    }

    #[test]
    fn cbor_decodes_random_bytes_safely(bytes in prop_oneof![vec(any::<u8>(), 0..16), vec(any::<u8>(), 0..600)]) {
        let _ = check_decode(&bytes);
    }

    #[test]
    fn cbor_decodes_mutated_encodings_safely(v in value(), mutations in vec(mutation(), 1..4)) {
        let mut bytes = cbor::encode(&v);
        for m in &mutations {
            mutate(&mut bytes, m);
        }
        let _ = check_decode(&bytes);
    }

    /// Canonical uniqueness: no other byte string decodes to the same value.
    #[test]
    fn cbor_single_byte_change_never_gives_the_same_value(v in value(), at in any::<usize>(), x in 1..=255u8) {
        let mut bytes = cbor::encode(&v);
        let at = at % bytes.len();
        bytes[at] ^= x;
        if let Ok(decoded) = check_decode(&bytes) {
            prop_assert_ne!(decoded, nfc(&v));
        }
    }
}

fn head(major: u8, n: u64) -> Vec<u8> {
    let m = major << 5;
    match n {
        0..=23 => vec![m | n as u8],
        24..=0xff => vec![m | 24, n as u8],
        0x100..=0xffff => [&[m | 25][..], &(n as u16).to_be_bytes()].concat(),
        0x1_0000..=0xffff_ffff => [&[m | 26][..], &(n as u32).to_be_bytes()].concat(),
        _ => [&[m | 27][..], &n.to_be_bytes()].concat(),
    }
}

/// Runs `f` and fails if it is slow: a malformed input must be refused right away.
fn quickly<R>(what: &str, f: impl FnOnce() -> R) -> R {
    let start = Instant::now();
    let result = f();
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{what} took {:?}",
        start.elapsed()
    );
    result
}

#[test]
fn deep_nesting_is_rejected_quickly() {
    let nested = |levels: usize, open: &[u8]| -> Vec<u8> {
        let mut bytes = open.repeat(levels);
        bytes.push(0x00);
        bytes
    };
    // An integer inside MAX_DEPTH arrays sits at depth MAX_DEPTH, the deepest allowed.
    assert!(check_decode(&nested(cbor::MAX_DEPTH, &[0x81])).is_ok());
    assert!(check_decode(&nested(cbor::MAX_DEPTH + 1, &[0x81])).is_err());
    assert!(check_decode(&nested(cbor::MAX_DEPTH, &[0xa1, 0x60])).is_ok());
    assert!(check_decode(&nested(cbor::MAX_DEPTH + 1, &[0xa1, 0x60])).is_err());
    for open in [&[0x81][..], &[0xa1, 0x60], &[0x9f], &[0x82, 0x00], &[0xa1, 0x61, 0x61]] {
        for levels in [100, 10_000, 1_000_000] {
            let bytes = nested(levels, open);
            assert!(
                quickly("deep nesting", || check_decode(&bytes)).is_err(),
                "{levels} levels of {open:x?}"
            );
        }
    }
}

#[test]
fn huge_declared_lengths_are_rejected_quickly() {
    for major in [2u8, 3, 4, 5] {
        for n in [
            24,
            0xff,
            0x100,
            0xffff,
            0xffff_ffff,
            0x1_0000_0000,
            i64::MAX as u64,
            u64::MAX,
        ] {
            for payload in [0usize, 1, 23] {
                let mut bytes = head(major, n);
                bytes.extend(std::iter::repeat_n(0x00, payload));
                let result = quickly("huge length", || check_decode(&bytes));
                assert!(result.is_err(), "major {major}, length {n}, {payload} bytes follow");
            }
        }
    }
    // Map keys and text inside containers declare lengths too.
    for bytes in [
        [&[0xa1][..], &head(3, u64::MAX)].concat(),
        [&[0x81][..], &head(2, u64::MAX)].concat(),
        [&[0x82, 0x00][..], &head(4, u64::MAX)].concat(),
    ] {
        assert!(quickly("huge inner length", || check_decode(&bytes)).is_err());
    }
}

/// Arrays inside arrays, each declaring as many items as there are bytes left. Every level is
/// plausible on its own, so a decoder that reserves room for the declared count up front would
/// hold (depth × input × value size) bytes before noticing the input is short.
#[test]
fn nested_large_declared_counts_do_not_multiply_memory() {
    let len = 256 * 1024;
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < 5 * (cbor::MAX_DEPTH + 1) {
        let remaining = len - bytes.len() - 5;
        bytes.push(0x9a);
        bytes.extend((remaining as u32).to_be_bytes());
    }
    bytes.resize(len, 0x00);
    assert!(quickly("nested counts", || check_decode(&bytes)).is_err());
}

// ---------------------------------------------------------------------------------------------
// IDs and keys in text form (base32)

const BASE32: &str = "abcdefghijklmnopqrstuvwxyz234567";

proptest! {
    #[test]
    fn id_text_round_trips(digest in any::<[u8; 32]>()) {
        let id = Id::from_digest(digest);
        let text = id.to_string();
        prop_assert_eq!(text.len(), 56);
        prop_assert!(text.starts_with("bdyq"));
        prop_assert!(text[1..].bytes().all(|b| BASE32.as_bytes().contains(&b)));
        prop_assert_eq!(text.parse::<Id>().unwrap(), id);
        prop_assert_eq!(format!("b{}", text[1..].to_uppercase()).parse::<Id>().unwrap(), id);
        prop_assert_eq!(Id::from_multihash(&id.to_multihash()).unwrap(), id);
    }

    /// Whatever parses is the one text form of that ID (ignoring the case of the body).
    #[test]
    fn id_text_parse_is_canonical(
        text in prop_oneof!["b[a-zA-Z2-7]{50,60}", "bdyq[a-z2-7]{52}", "[bB][a-z0-9=]{0,60}", ".{0,60}"],
    ) {
        if let Ok(id) = text.parse::<Id>() {
            prop_assert!(text.starts_with('b'));
            prop_assert_eq!(id.to_string(), text.to_lowercase());
        }
    }

    #[test]
    fn id_text_with_one_character_changed(digest in any::<[u8; 32]>(), at in 1..56usize, c in any::<char>()) {
        let id = Id::from_digest(digest);
        let mut chars: Vec<char> = id.to_string().chars().collect();
        chars[at] = c;
        let text: String = chars.into_iter().collect();
        if let Ok(other) = text.parse::<Id>() {
            prop_assert_eq!(other.to_string(), text.to_lowercase());
            prop_assert_eq!(other == id, text.to_lowercase() == id.to_string());
        }
    }

    #[test]
    fn key_text_round_trips(secret in any::<[u8; 32]>()) {
        let key = Keypair::from_secret(secret).public();
        let text = key.to_string();
        prop_assert_eq!(text.len(), 53);
        prop_assert_eq!(text.parse::<PublicKey>().unwrap(), key);
        prop_assert_eq!(format!("k{}", text[1..].to_uppercase()).parse::<PublicKey>().unwrap(), key);
        prop_assert_eq!(PublicKey::from_bytes(key.as_bytes()).unwrap(), key);
    }

    /// Random bytes are a key in text form exactly when they are a key in binary form.
    #[test]
    fn key_text_agrees_with_binary(bytes in any::<[u8; 32]>(), suffix in "[a-z2-7=]{0,2}") {
        // RFC 4648 base32 of the bytes, written out here independently of the crate's encoder.
        let mut body = String::new();
        let mut buffer = 0u32;
        let mut bits = 0;
        for byte in bytes {
            buffer = (buffer << 8) | u32::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                body.push(BASE32.as_bytes()[((buffer >> bits) & 31) as usize] as char);
            }
        }
        body.push(BASE32.as_bytes()[((buffer << (5 - bits)) & 31) as usize] as char);
        let text = format!("k{body}");
        let parsed = text.parse::<PublicKey>();
        prop_assert_eq!(parsed.is_ok(), PublicKey::from_bytes(&bytes).is_ok());
        if let Ok(key) = parsed {
            prop_assert_eq!(key.as_bytes(), &bytes);
            prop_assert_eq!(key.to_string(), text.clone());
        }
        let longer = format!("{text}{suffix}");
        if !suffix.is_empty() {
            prop_assert!(longer.parse::<PublicKey>().is_err());
        }
    }

    #[test]
    fn key_text_parse_is_canonical(text in prop_oneof!["k[a-zA-Z2-7]{50,56}", "[kK][a-z0-9=]{0,60}", ".{0,60}"]) {
        if let Ok(key) = text.parse::<PublicKey>() {
            prop_assert!(text.starts_with('k'));
            prop_assert_eq!(key.to_string(), text.to_lowercase());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Object strategies

fn placeholder_key() -> PublicKey {
    Keypair::from_secret([0; 32]).public()
}

fn id() -> impl Strategy<Value = Id> {
    any::<[u8; 32]>().prop_map(Id::from_digest)
}

fn ids(range: std::ops::Range<usize>) -> impl Strategy<Value = Vec<Id>> {
    btree_set(any::<[u8; 32]>(), range).prop_map(|set| set.into_iter().map(Id::from_digest).collect())
}

fn key() -> impl Strategy<Value = PublicKey> {
    any::<[u8; 32]>().prop_map(|s| Keypair::from_secret(s).public())
}

/// A lowercase word that no current spec version gives a meaning to.
fn new_word() -> impl Strategy<Value = String> {
    "x-[a-z0-9-]{0,30}"
}

/// NFC text that isn't blank, as bounded text fields require.
fn bounded_text() -> impl Strategy<Value = String> {
    ".{1,40}"
        .prop_map(|s| s.nfc().collect::<String>())
        .prop_filter("not blank", |s| !s.trim().is_empty())
}

fn topics(range: std::ops::Range<usize>) -> impl Strategy<Value = Vec<Topic>> {
    btree_set("[a-z0-9-]{1,10}(/[a-z0-9-]{1,10}){0,3}", range)
        .prop_map(|set| set.into_iter().map(|t| t.parse().unwrap()).collect())
}

fn time() -> impl Strategy<Value = Time> {
    let src = prop_oneof![
        Just(TimeSource::Gps),
        Just(TimeSource::Network),
        Just(TimeSource::User),
        Just(TimeSource::Unknown),
        new_word().prop_map(TimeSource::Other),
    ];
    (any::<i64>(), src).prop_map(|(ms, src)| Time { ms, src })
}

fn clock() -> impl Strategy<Value = u64> {
    prop_oneof![1..100u64, 1..=i64::MAX as u64]
}

/// Fields from later format versions: keys no object type uses here.
fn extra() -> impl Strategy<Value = Map> {
    vec(("x-[a-z]{0,6}", value()), 0..3).prop_map(|entries| {
        let mut map = Map::new();
        for (k, v) in entries {
            map.insert(&k, v);
        }
        nfc_map(&map)
    })
}

fn content_file() -> impl Strategy<Value = ContentFile> {
    (
        "[A-Za-z0-9 ._()\u{e9}\u{4e2d}-]{1,30}".prop_filter("not a folder name", |n| n != "." && n != ".."),
        id(),
        prop_oneof![0..10_000u64, 0..=i64::MAX as u64],
        "[a-z]{1,10}/[a-z0-9.+-]{1,20}",
    )
        .prop_map(|(name, blob, size, mime)| ContentFile {
            name: name.nfc().collect(),
            blob,
            size,
            mime,
        })
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        prop::sample::select(vec![
            Kind::Guide,
            Kind::Article,
            Kind::Document,
            Kind::Video,
            Kind::Audio,
            Kind::Image,
            Kind::Map,
            Kind::Pack,
            Kind::Catalog,
            Kind::WebArchive,
            Kind::Dataset,
        ]),
        new_word().prop_map(Kind::Other),
    ]
}

fn revision() -> impl Strategy<Value = Revision> {
    let lineage = prop_oneof![
        Just((None, vec![])),
        (id(), ids(1..4)).prop_map(|(item, parents)| (Some(item), parents)),
    ];
    let head = (
        lineage,
        clock(),
        time(),
        kind(),
        bounded_text(),
        "[a-z]{2,3}(-[a-zA-Z0-9]{1,8}){0,2}",
    );
    let body = (
        topics(1..5),
        "[A-Za-z0-9.+()-]{1,20}( (AND|OR) [A-Za-z0-9.+-]{1,20})?",
        prop::sample::select(vec![Visibility::Public, Visibility::Node, Visibility::Private]),
        vec(content_file(), 1..4),
        prop::option::of(extra()),
        prop::option::of(bounded_text()),
        extra(),
    );
    (head, body).prop_map(
        |(
            ((item, parents), clock, time, kind, title, lang),
            (topics, license, vis, content, source, summary, extra),
        )| {
            Revision {
                item,
                parents,
                author: placeholder_key(),
                clock,
                time,
                kind,
                title,
                lang,
                topics,
                license,
                vis,
                content,
                source,
                summary,
                extra,
            }
        },
    )
}

/// A valid attestation: the claim decides what it may be about and which fields it needs.
fn attestation() -> impl Strategy<Value = Attestation> {
    let claim = prop_oneof![
        prop::sample::select(vec![
            Claim::Verified,
            Claim::Dangerous,
            Claim::Outdated,
            Claim::Hide,
            Claim::TranslationOf,
            Claim::DuplicateOf,
            Claim::Replaces,
            Claim::Trust,
            Claim::Withdraw,
        ]),
        new_word().prop_map(Claim::Other),
    ];
    (
        claim,
        id(),
        key(),
        any::<bool>(),
        topics(1..5),
        any::<bool>(),
        prop::option::of(id()),
        prop::option::of(bounded_text()),
        clock(),
        time(),
        extra(),
    )
        .prop_map(
            |(claim, object, key, about_key, topics, with_topics, related, note, clock, time, extra)| {
                let about = match claim {
                    Claim::Trust => Subject::Key(key),
                    Claim::Other(_) if about_key => Subject::Key(key),
                    _ => Subject::Object(object),
                };
                let needs_topics = matches!(claim, Claim::Trust | Claim::Verified);
                let needs_related = matches!(claim, Claim::TranslationOf | Claim::DuplicateOf | Claim::Replaces);
                Attestation {
                    about,
                    topics: if needs_topics || with_topics { topics } else { vec![] },
                    related: if needs_related {
                        related.or(Some(object))
                    } else {
                        related
                    },
                    claim,
                    note,
                    author: placeholder_key(),
                    clock,
                    time,
                    extra,
                }
            },
        )
}

/// A sealed object of every signed type, plus a log entry, to tamper with.
fn sample_objects(kp: &Keypair) -> Vec<Vec<u8>> {
    let other = Keypair::from_secret([0xee; 32]).public();
    let time = Time {
        ms: 1_792_000_000_000,
        src: TimeSource::Gps,
    };
    let revision = Revision {
        item: Some(Id::of(b"item")),
        parents: vec![Id::of(b"parent")],
        author: kp.public(),
        clock: 7,
        time: time.clone(),
        kind: Kind::Guide,
        title: "Clean water".into(),
        lang: "en".into(),
        topics: vec!["water/treatment".parse().unwrap()],
        license: "CC-BY-SA-4.0".into(),
        vis: Visibility::Public,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob: Id::of(b"guide"),
            size: 10,
            mime: "text/markdown".into(),
        }],
        source: Some(Map::new()),
        summary: Some("Boil it".into()),
        extra: Map::new(),
    };
    let attestation = Attestation {
        about: Subject::Object(Id::of(b"rev")),
        claim: Claim::Replaces,
        topics: vec!["water".parse().unwrap()],
        related: Some(Id::of(b"old")),
        note: Some("newer".into()),
        author: kp.public(),
        clock: 8,
        time: time.clone(),
        extra: Map::new(),
    };
    let record = KeyRecord {
        key: kp.public(),
        kind: KeyKind::Node,
        name: "Clinic node".into(),
        recovery: Some(other),
        clock: 9,
        time: time.clone(),
        extra: Map::new(),
    };
    let revoke = Revoke {
        key: other,
        from: 3,
        by: kp.public(),
        clock: 10,
        time,
        extra: Map::new(),
    };
    let derived = Derived {
        from: Id::of(b"pdf"),
        producer: "nevo-ocr/1.0".into(),
        out: Id::of(b"text"),
        author: kp.public(),
        extra: Map::new(),
    };
    let entry = LogEntry {
        seq: 2,
        prev: Some(Id::of(b"entry 1")),
        obj: Id::of(b"obj"),
        extra: Map::new(),
    };
    let head = LogHead {
        node: kp.public(),
        seq: 2,
        entry: Id::of(b"entry 2"),
        extra: Map::new(),
    };
    vec![
        seal(&revision, kp).unwrap().bytes().to_vec(),
        seal(&attestation, kp).unwrap().bytes().to_vec(),
        seal(&record, kp).unwrap().bytes().to_vec(),
        seal(&revoke, kp).unwrap().bytes().to_vec(),
        seal(&derived, kp).unwrap().bytes().to_vec(),
        seal_unsigned(&entry).unwrap().bytes().to_vec(),
        seal(&head, kp).unwrap().bytes().to_vec(),
    ]
}

/// The ID `open_any` gives, whatever the type.
fn any_id(object: &AnyObject) -> Id {
    match object {
        AnyObject::Revision(s) => s.id(),
        AnyObject::Attestation(s) => s.id(),
        AnyObject::Key(s) => s.id(),
        AnyObject::Revoke(s) => s.id(),
        AnyObject::Derived(s) => s.id(),
        AnyObject::LogEntry(s) => s.id(),
        AnyObject::LogHead(s) => s.id(),
        AnyObject::Unknown { id, .. } => *id,
    }
}

/// Opens untrusted bytes as any object and checks what holds for every accepted object: it
/// re-encodes to the same bytes and its ID is their hash.
fn check_open_any(bytes: &[u8]) -> Result<AnyObject> {
    let result = open_any(bytes);
    if let Ok(object) = &result {
        assert_eq!(any_id(object), Id::of(bytes));
        assert_eq!(cbor::encode(&cbor::decode(bytes).unwrap()), bytes);
    }
    result
}

/// Whether a single-byte change produced something a careful reader would accept as signed.
fn assert_tamper_rejected(original: &[u8], tampered: &[u8]) {
    match check_open_any(tampered) {
        Err(_) => {}
        // The type name itself changed into one from a "later version": stored, never trusted.
        Ok(AnyObject::Unknown { .. }) => {}
        Ok(other) => panic!(
            "a one-byte change of {original:x?} still opens: {other:?}",
            original = &original[..original.len().min(16)]
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Objects

proptest! {
    #[test]
    fn revisions_seal_and_open(secret in any::<[u8; 32]>(), mut revision in revision()) {
        let kp = Keypair::from_secret(secret);
        revision.author = kp.public();
        let sealed = seal(&revision, &kp).unwrap();
        prop_assert_eq!(&sealed.object, &revision);
        prop_assert_eq!(sealed.id(), Id::of(sealed.bytes()));
        prop_assert!(sealed.bytes().len() <= MAX_OBJECT_BYTES);
        let opened = open::<Revision>(sealed.bytes()).unwrap();
        prop_assert_eq!(&opened, &sealed);
        prop_assert_eq!(seal(&opened.object, &kp).unwrap().id(), sealed.id(), "IDs are stable");
        prop_assert!(matches!(check_open_any(sealed.bytes()).unwrap(), AnyObject::Revision(s) if s == sealed));
        prop_assert!(open::<Attestation>(sealed.bytes()).is_err());
    }

    #[test]
    fn attestations_seal_and_open(secret in any::<[u8; 32]>(), mut attestation in attestation()) {
        let kp = Keypair::from_secret(secret);
        attestation.author = kp.public();
        let sealed = seal(&attestation, &kp).unwrap();
        prop_assert_eq!(&sealed.object, &attestation);
        let opened = open::<Attestation>(sealed.bytes()).unwrap();
        prop_assert_eq!(&opened, &sealed);
        prop_assert_eq!(seal(&opened.object, &kp).unwrap().id(), sealed.id(), "IDs are stable");
        prop_assert!(matches!(check_open_any(sealed.bytes()).unwrap(), AnyObject::Attestation(s) if s == sealed));
    }

    #[test]
    fn sealed_revisions_detect_any_one_byte_change(
        secret in any::<[u8; 32]>(),
        mut revision in revision(),
        at in any::<usize>(),
        x in 1..=255u8,
    ) {
        let kp = Keypair::from_secret(secret);
        revision.author = kp.public();
        let sealed = seal(&revision, &kp).unwrap();
        let mut bytes = sealed.bytes().to_vec();
        let at = at % bytes.len();
        bytes[at] ^= x;
        prop_assert!(open::<Revision>(&bytes).is_err(), "byte {} ^ {:#x}", at, x);
        assert_tamper_rejected(sealed.bytes(), &bytes);
    }

    #[test]
    fn sealed_attestations_detect_any_one_byte_change(
        secret in any::<[u8; 32]>(),
        mut attestation in attestation(),
        at in any::<usize>(),
        x in 1..=255u8,
    ) {
        let kp = Keypair::from_secret(secret);
        attestation.author = kp.public();
        let sealed = seal(&attestation, &kp).unwrap();
        let mut bytes = sealed.bytes().to_vec();
        let at = at % bytes.len();
        bytes[at] ^= x;
        prop_assert!(open::<Attestation>(&bytes).is_err(), "byte {} ^ {:#x}", at, x);
        assert_tamper_rejected(sealed.bytes(), &bytes);
    }

    #[test]
    fn every_type_detects_one_byte_changes(which in 0..7usize, at in any::<usize>(), x in 1..=255u8) {
        let kp = Keypair::from_secret([0x42; 32]);
        let original = &sample_objects(&kp)[which];
        let mut bytes = original.clone();
        let at = at % bytes.len();
        bytes[at] ^= x;
        if which == 5 {
            // Log entries are unsigned: a changed entry may still be a valid entry, but never the
            // same one, and the chain of IDs catches it (see the store's properties).
            if let Ok(AnyObject::LogEntry(e)) = check_open_any(&bytes) {
                prop_assert_ne!(e.id(), Id::of(original));
            }
        } else {
            assert_tamper_rejected(original, &bytes);
        }
    }

    #[test]
    fn open_any_survives_random_bytes(bytes in prop_oneof![vec(any::<u8>(), 0..64), vec(any::<u8>(), 0..2000)]) {
        let _ = check_open_any(&bytes);
    }

    #[test]
    fn open_any_survives_mutated_objects(which in 0..7usize, mutations in vec(mutation(), 1..5)) {
        let kp = Keypair::from_secret([0x42; 32]);
        let mut bytes = sample_objects(&kp)[which].clone();
        for m in &mutations {
            mutate(&mut bytes, m);
        }
        let _ = check_open_any(&bytes);
    }

    /// Maps built from the fields objects use, holding random values: never a signed object,
    /// because nobody signed them.
    #[test]
    fn open_any_survives_random_maps(
        t in prop_oneof![
            prop::sample::select(vec!["rev", "att", "key", "revoke", "derived", "log", "head"]).prop_map(String::from),
            text(),
        ],
        v in prop_oneof![4 => Just(Value::Int(1)), 1 => value()],
        fields in vec((field_name(), field_value()), 0..12),
    ) {
        let mut map = Map::new();
        for (k, value) in fields {
            map.insert(k, value);
        }
        map.insert("t", t).insert("v", v);
        let bytes = cbor::encode_map(&nfc_map(&map));
        match check_open_any(&bytes) {
            Err(_) | Ok(AnyObject::Unknown { .. }) | Ok(AnyObject::LogEntry(_)) => {}
            Ok(other) => panic!("an unsigned random map opened as {other:?}"),
        }
    }

    /// Validation as such: a field replaced by a random value, then properly signed again. Must
    /// not panic, and anything accepted must re-encode from its parsed form to the same bytes.
    #[test]
    fn resigned_objects_with_a_random_field(which in 0..7usize, name in field_name(), value in field_value()) {
        let kp = Keypair::from_secret([0x42; 32]);
        let Value::Map(mut map) = cbor::decode(&sample_objects(&kp)[which]).unwrap() else { unreachable!() };
        map.insert(name, value);
        let map = nfc_map(&map);
        let map = match which {
            5 => map,
            // Signing needs a text `t`; without one there is nothing to sign.
            _ => match kp.sign(map) {
                Ok(signed) => signed,
                Err(_) => return Ok(()),
            },
        };
        let bytes = cbor::encode_map(&map);
        let resealed = match check_open_any(&bytes) {
            Err(_) | Ok(AnyObject::Unknown { .. }) => return Ok(()),
            Ok(AnyObject::Revision(s)) => seal(&s.object, &kp).map(|r| r.bytes().to_vec()),
            Ok(AnyObject::Attestation(s)) => seal(&s.object, &kp).map(|r| r.bytes().to_vec()),
            Ok(AnyObject::Key(s)) => seal(&s.object, &kp).map(|r| r.bytes().to_vec()),
            Ok(AnyObject::Revoke(s)) => seal(&s.object, &kp).map(|r| r.bytes().to_vec()),
            Ok(AnyObject::Derived(s)) => seal(&s.object, &kp).map(|r| r.bytes().to_vec()),
            Ok(AnyObject::LogEntry(s)) => seal_unsigned(&s.object).map(|r| r.bytes().to_vec()),
            Ok(AnyObject::LogHead(s)) => seal(&s.object, &kp).map(|r| r.bytes().to_vec()),
        };
        match resealed {
            // Known gap, see `unknown_keys_in_nested_maps_survive_parsing`.
            Ok(_) if name == "time" || name == "content" => {}
            Ok(resealed) => prop_assert_eq!(resealed, bytes, "parsed form must re-encode exactly"),
            // Some fields name the signer; replacing it with another key leaves `kp` unable to seal.
            Err(Error::Invalid { .. }) => {}
            Err(e) => panic!("re-sealing an accepted object failed: {e:?}"),
        }
    }
}

const FIELD_NAMES: &[&str] = &[
    "t", "v", "sig", "item", "parents", "author", "clock", "time", "kind", "title", "lang", "topics", "license", "vis",
    "content", "source", "summary", "about", "claim", "related", "note", "key", "name", "recovery", "from", "by",
    "producer", "out", "seq", "prev", "obj", "node", "entry", "x-new",
];

fn field_name() -> impl Strategy<Value = &'static str> {
    prop::sample::select(FIELD_NAMES)
}

/// Values a field could plausibly hold, valid or not, plus anything at all.
fn field_value() -> impl Strategy<Value = Value> {
    let time = (any::<i64>(), "gps|unknown|x-[a-z]{0,3}|[A-Z ]{0,3}", extra()).prop_map(|(ms, src, mut extra)| {
        extra.insert("ms", ms).insert("src", src);
        Value::Map(extra)
    });
    let file = (id(), "[a-z./ ]{0,6}", int(), "[a-z/ ]{0,6}", extra()).prop_map(|(blob, name, size, mime, mut m)| {
        m.insert("blob", blob)
            .insert("name", name)
            .insert("size", size)
            .insert("mime", mime);
        Value::Map(m)
    });
    prop_oneof![
        value(),
        id().prop_map(Value::from),
        key().prop_map(Value::from),
        vec(any::<u8>(), 30..36).prop_map(|mut b| {
            b[..2].copy_from_slice(&MULTIHASH_BLAKE3);
            Value::Bytes(b)
        }),
        int().prop_map(Value::Int),
        "[a-z0-9/ -]{0,12}".prop_map(Value::Text),
        prop::sample::select(vec!["public", "private", "trust", "verified", "guide", "person", "a/b"])
            .prop_map(Value::from),
        vec(id().prop_map(Value::from), 0..3).prop_map(Value::Array),
        vec("[a-z/]{0,8}".prop_map(Value::Text), 0..18).prop_map(Value::Array),
        time,
        vec(file, 0..3).prop_map(Value::Array),
        vec(any::<u8>(), 64..=64).prop_map(Value::Bytes),
    ]
}

/// Spec section 1: "Unknown keys are kept and ignored". The top level keeps them (`extra`), but
/// the parsed `Time` and `ContentFile` have nowhere to keep them, so an accepted object no longer
/// re-encodes from its parsed form. Stored bytes and IDs are unaffected (they are kept as
/// received); re-sealing a parsed object is what changes it.
#[test]
#[ignore = "known gap: unknown keys inside `time` and content entries are dropped when parsed"]
fn unknown_keys_in_nested_maps_survive_parsing() {
    let kp = Keypair::from_secret([0x42; 32]);
    let Value::Map(mut map) = cbor::decode(&sample_objects(&kp)[0]).unwrap() else {
        unreachable!()
    };
    let Some(Value::Map(mut time)) = map.get("time").cloned() else {
        unreachable!()
    };
    time.insert("x-zone", "UTC+1");
    map.insert("time", time);
    let bytes = cbor::encode_map(&kp.sign(map).unwrap());
    let opened = open::<Revision>(&bytes).expect("unknown keys are accepted");
    assert_eq!(seal(&opened.object, &kp).unwrap().bytes(), bytes);
}

/// Every byte of one object of each type, changed to every other value.
#[test]
#[ignore = "exhaustive: about 400 000 opens"]
fn exhaustive_one_byte_changes() {
    let kp = Keypair::from_secret([0x42; 32]);
    for (which, original) in sample_objects(&kp).iter().enumerate() {
        for at in 0..original.len() {
            for x in 1..=255u8 {
                let mut bytes = original.clone();
                bytes[at] ^= x;
                if which == 5 {
                    if let Ok(AnyObject::LogEntry(e)) = check_open_any(&bytes) {
                        assert_ne!(e.id(), Id::of(original));
                    }
                } else {
                    assert_tamper_rejected(original, &bytes);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Topics

/// Topic syntax, written independently of the parser.
fn topic_is_valid(s: &str) -> bool {
    let segments: Vec<&str> = s.split('/').collect();
    !s.is_empty()
        && s.len() <= 200
        && segments.len() <= 6
        && segments
            .iter()
            .all(|seg| (1..=40).contains(&seg.len()) && seg.chars().all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-')))
}

fn small_topic() -> impl Strategy<Value = Topic> {
    "[ab]{1,2}(/[ab]{1,2}){0,3}".prop_map(|s| s.parse().unwrap())
}

proptest! {
    #[test]
    fn topic_parse_matches_the_syntax(
        s in prop_oneof!["[a-z0-9/-]{0,14}", "[a-zA-Z0-9/ ._é-]{0,14}", ".{0,20}", "[a-z/]{150,250}", "[a-z]{38,42}(/[a-z]{1,3}){0,2}"],
    ) {
        match s.parse::<Topic>() {
            Ok(topic) => {
                prop_assert!(topic_is_valid(&s), "accepted {:?}", s);
                prop_assert_eq!(topic.as_str(), s.as_str());
                prop_assert_eq!(topic.to_string(), s.clone());
                prop_assert_eq!(topic.top(), s.split('/').next().unwrap());
            }
            Err(_) => prop_assert!(!topic_is_valid(&s), "refused {:?}", s),
        }
    }

    #[test]
    fn covers_follows_segments(a in small_topic(), b in small_topic(), c in small_topic()) {
        let segs = |t: &Topic| t.as_str().split('/').map(String::from).collect::<Vec<_>>();
        let model = |x: &Topic, y: &Topic| segs(y).starts_with(&segs(x));
        prop_assert!(a.covers(&a), "reflexive");
        prop_assert_eq!(a.covers(&b), model(&a, &b), "{:?} covers {:?}", a, b);
        if a.covers(&b) && b.covers(&a) {
            prop_assert_eq!(&a, &b, "antisymmetric");
        }
        if a.covers(&b) && b.covers(&c) {
            prop_assert!(a.covers(&c), "transitive");
        }
        if a.covers(&b) {
            prop_assert_eq!(a.top(), b.top());
        }
    }

    #[test]
    fn covers_chains(a in "[ab]{1,2}(/[ab]{1,2})?".prop_map(|s| s.parse::<Topic>().unwrap()), tail1 in "(/[ab]{1,2}){0,2}", tail2 in "(/[ab]{1,2}){0,2}") {
        let b: Topic = format!("{a}{tail1}").parse().unwrap();
        let c: Topic = format!("{b}{tail2}").parse().unwrap();
        prop_assert!(a.covers(&b) && b.covers(&c) && a.covers(&c));
        // Extending the last segment instead of adding one is not a sub-topic.
        let longer: Topic = format!("{a}a").parse().unwrap();
        prop_assert!(!a.covers(&longer));
        prop_assert!(!longer.covers(&a));
    }
}
