//! The object types of spec sections 2 and 4, and sealing (encode + sign) and opening
//! (decode + validate + verify) them.

use crate::cbor::{self, Map, Value};
use crate::error::{Error, Result};
use crate::id::{Id, PublicKey};
use crate::sign::{self, Keypair};
use crate::topic::Topic;

pub const FORMAT_VERSION: u64 = 1;

/// Largest encoded object accepted. Real objects are well under 10 KB.
pub const MAX_OBJECT_BYTES: usize = 1 << 20;

const MAX_TITLE: usize = 500;
const MAX_NOTE: usize = 4000;
const MAX_LIST: usize = 1000;

/// An object type: its CBOR fields and who must sign it.
pub trait Object: Sized {
    const TYPE: &'static str;
    /// The object's fields, except `t`, `v` and `sig`.
    fn to_map(&self) -> Map;
    /// Reads and validates the fields. `t`, `v` and `sig` are checked by [`open`].
    fn from_map(map: &Map) -> Result<Self>;
    /// The key that must have signed the object, or `None` for unsigned objects.
    fn signer(&self) -> Option<PublicKey>;
}

/// An object together with its canonical bytes and ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sealed<T> {
    pub object: T,
    bytes: Vec<u8>,
    id: Id,
}

impl<T> Sealed<T> {
    pub fn id(&self) -> Id {
        self.id
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Encodes `object` and signs it with `key`, which must be the object's signer.
pub fn seal<T: Object>(object: &T, key: &Keypair) -> Result<Sealed<T>> {
    match object.signer() {
        Some(signer) if signer == key.public() => {}
        Some(_) => return Err(Error::invalid("sig", "this key is not the object's signer")),
        None => return Err(Error::invalid("sig", "this object type is not signed")),
    }
    let map = key.sign(with_header::<T>(object.to_map()))?;
    // Re-open the bytes so that sealing and opening can never disagree.
    open(&cbor::encode_map(&map))
}

/// Encodes an unsigned object (a log entry).
pub fn seal_unsigned<T: Object>(object: &T) -> Result<Sealed<T>> {
    if object.signer().is_some() {
        return Err(Error::Missing("sig"));
    }
    open(&cbor::encode_map(&with_header::<T>(object.to_map())))
}

fn with_header<T: Object>(mut map: Map) -> Map {
    map.insert("t", T::TYPE).insert("v", FORMAT_VERSION);
    map
}

/// Decodes, validates and verifies an object of type `T`.
pub fn open<T: Object>(bytes: &[u8]) -> Result<Sealed<T>> {
    let (t, map) = decode_object(bytes)?;
    if t != T::TYPE {
        return Err(Error::WrongType {
            expected: T::TYPE,
            found: t,
        });
    }
    let object = T::from_map(&map)?;
    match object.signer() {
        Some(signer) => sign::verify(&map, &signer)?,
        None if map.contains("sig") => return Err(Error::invalid("sig", "this object type is not signed")),
        None => {}
    }
    Ok(Sealed {
        object,
        bytes: bytes.to_vec(),
        id: Id::of(bytes),
    })
}

/// Any object, for code that receives bytes without knowing their type (sync, bundles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnyObject {
    Revision(Sealed<Revision>),
    Attestation(Sealed<Attestation>),
    Key(Sealed<KeyRecord>),
    Revoke(Sealed<Revoke>),
    Derived(Sealed<Derived>),
    LogEntry(Sealed<LogEntry>),
    LogHead(Sealed<LogHead>),
    /// A type from a newer format version. It can be stored and passed on but not checked.
    Unknown {
        t: String,
        id: Id,
    },
}

pub fn open_any(bytes: &[u8]) -> Result<AnyObject> {
    let (t, _) = decode_object(bytes)?;
    Ok(match t.as_str() {
        Revision::TYPE => AnyObject::Revision(open(bytes)?),
        Attestation::TYPE => AnyObject::Attestation(open(bytes)?),
        KeyRecord::TYPE => AnyObject::Key(open(bytes)?),
        Revoke::TYPE => AnyObject::Revoke(open(bytes)?),
        Derived::TYPE => AnyObject::Derived(open(bytes)?),
        LogEntry::TYPE => AnyObject::LogEntry(open(bytes)?),
        LogHead::TYPE => AnyObject::LogHead(open(bytes)?),
        _ => AnyObject::Unknown { t, id: Id::of(bytes) },
    })
}

fn decode_object(bytes: &[u8]) -> Result<(String, Map)> {
    if bytes.len() > MAX_OBJECT_BYTES {
        return Err(Error::TooLarge);
    }
    let map = match cbor::decode(bytes)? {
        Value::Map(map) => map,
        _ => return Err(Error::Cbor("an object must be a map")),
    };
    let t = match map.get("t") {
        Some(Value::Text(t)) => t.clone(),
        Some(_) => return Err(Error::invalid("t", "must be text")),
        None => return Err(Error::Missing("t")),
    };
    match map.get("v") {
        Some(Value::Int(1)) => {}
        Some(Value::Int(v)) => return Err(Error::UnsupportedVersion(*v)),
        Some(_) => return Err(Error::invalid("v", "must be an integer")),
        None => return Err(Error::Missing("v")),
    }
    Ok((t, map))
}

// ---------------------------------------------------------------------------------------------
// Shared field types

/// Best-guess wall-clock time. Ordering never depends on it; see `clock`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Time {
    /// Unix time in milliseconds.
    pub ms: i64,
    pub src: TimeSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeSource {
    Gps,
    Network,
    User,
    Unknown,
    /// A source added by a later spec version; treated like `Unknown`.
    Other(String),
}

impl TimeSource {
    pub fn as_str(&self) -> &str {
        match self {
            TimeSource::Gps => "gps",
            TimeSource::Network => "network",
            TimeSource::User => "user",
            TimeSource::Unknown => "unknown",
            TimeSource::Other(s) => s,
        }
    }
}

impl Time {
    fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("ms", self.ms).insert("src", self.src.as_str());
        m.into()
    }

    fn from_value(value: &Value) -> Result<Time> {
        let Value::Map(m) = value else {
            return Err(Error::invalid("time", "must be a map"));
        };
        let f = Fields(m);
        let src = match f.text("src")?.as_str() {
            "gps" => TimeSource::Gps,
            "network" => TimeSource::Network,
            "user" => TimeSource::User,
            "unknown" => TimeSource::Unknown,
            other => TimeSource::Other(word("time", other)?),
        };
        Ok(Time { ms: f.int("ms")?, src })
    }
}

/// What a revision is. Newer format versions may add kinds, which arrive as `Other`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Guide,
    Article,
    Document,
    Video,
    Audio,
    Image,
    Map,
    Pack,
    Catalog,
    WebArchive,
    Dataset,
    Other(String),
}

impl Kind {
    pub fn as_str(&self) -> &str {
        match self {
            Kind::Guide => "guide",
            Kind::Article => "article",
            Kind::Document => "document",
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Image => "image",
            Kind::Map => "map",
            Kind::Pack => "pack",
            Kind::Catalog => "catalog",
            Kind::WebArchive => "web-archive",
            Kind::Dataset => "dataset",
            Kind::Other(s) => s,
        }
    }

    fn parse(s: &str) -> Result<Kind> {
        Ok(match s {
            "guide" => Kind::Guide,
            "article" => Kind::Article,
            "document" => Kind::Document,
            "video" => Kind::Video,
            "audio" => Kind::Audio,
            "image" => Kind::Image,
            "map" => Kind::Map,
            "pack" => Kind::Pack,
            "catalog" => Kind::Catalog,
            "web-archive" => Kind::WebArchive,
            "dataset" => Kind::Dataset,
            other => Kind::Other(word("kind", other)?),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// Shared with other nodes.
    Public,
    /// Stays on this node.
    Node,
    /// Never leaves this node and is hidden from other users.
    Private,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Node => "node",
            Visibility::Private => "private",
        }
    }
}

/// One file of a revision's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentFile {
    /// File name, without any folder part.
    pub name: String,
    pub blob: Id,
    pub size: u64,
    pub mime: String,
}

impl ContentFile {
    fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("name", self.name.as_str())
            .insert("blob", self.blob)
            .insert("size", self.size)
            .insert("mime", self.mime.as_str());
        m.into()
    }

    fn from_value(value: &Value) -> Result<ContentFile> {
        let Value::Map(m) = value else {
            return Err(Error::invalid("content", "entries must be maps"));
        };
        let f = Fields(m);
        let name = f.text("name")?;
        // Names become file names on export, so nothing that could escape a folder.
        let unsafe_name = name.is_empty()
            || name.len() > 255
            || name == "."
            || name == ".."
            || name.chars().any(|c| matches!(c, '/' | '\\' | ':') || c.is_control());
        if unsafe_name {
            return Err(Error::invalid("content", format!("unsafe file name `{name}`")));
        }
        let mime = f.text("mime")?;
        if !mime.contains('/') || mime.len() > 100 || !mime.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::invalid("content", format!("bad media type `{mime}`")));
        }
        Ok(ContentFile {
            name,
            blob: f.id("blob")?,
            size: f.uint("size")?,
            mime,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Revision

/// An immutable, signed version of an item (spec section 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    /// The item this revision belongs to; `None` for an item's first revision, whose own ID
    /// becomes the item's ID.
    pub item: Option<Id>,
    pub parents: Vec<Id>,
    pub author: PublicKey,
    pub clock: u64,
    pub time: Time,
    pub kind: Kind,
    pub title: String,
    /// BCP 47 language tag.
    pub lang: String,
    pub topics: Vec<Topic>,
    /// SPDX license identifier or expression.
    pub license: String,
    pub vis: Visibility,
    pub content: Vec<ContentFile>,
    /// Where the content came from (title, url, author, retrieved), for attribution.
    pub source: Option<Map>,
    pub summary: Option<String>,
    /// Fields from newer format versions, kept so re-encoding is exact.
    pub extra: Map,
}

const REVISION_FIELDS: &[&str] = &[
    "t", "v", "sig", "item", "parents", "author", "clock", "time", "kind", "title", "lang", "topics", "license", "vis",
    "content", "source", "summary",
];

impl Revision {
    /// The item's ID, given this revision's own ID.
    pub fn item_id(&self, own_id: Id) -> Id {
        self.item.unwrap_or(own_id)
    }
}

impl Object for Revision {
    const TYPE: &'static str = "rev";

    fn to_map(&self) -> Map {
        let mut m = self.extra.clone();
        m.insert_opt("item", self.item)
            .insert("parents", ids_value(&self.parents))
            .insert("author", self.author)
            .insert("clock", self.clock)
            .insert("time", self.time.to_value())
            .insert("kind", self.kind.as_str())
            .insert("title", self.title.as_str())
            .insert("lang", self.lang.as_str())
            .insert("topics", topics_value(&self.topics))
            .insert("license", self.license.as_str())
            .insert("vis", self.vis.as_str())
            .insert(
                "content",
                Value::Array(self.content.iter().map(ContentFile::to_value).collect()),
            )
            .insert_opt("source", self.source.clone())
            .insert_opt("summary", self.summary.as_deref());
        m
    }

    fn from_map(map: &Map) -> Result<Revision> {
        let f = Fields(map);
        let item = f.opt_id("item")?;
        let parents = f.ids("parents")?;
        if item.is_none() != parents.is_empty() {
            return Err(Error::invalid(
                "parents",
                "a first revision has no item and no parents; later ones have both",
            ));
        }
        let vis = match f.text("vis")?.as_str() {
            "public" => Visibility::Public,
            "node" => Visibility::Node,
            "private" => Visibility::Private,
            other => return Err(Error::invalid("vis", format!("unknown visibility `{other}`"))),
        };
        let content = f
            .array("content")?
            .iter()
            .map(ContentFile::from_value)
            .collect::<Result<Vec<_>>>()?;
        if content.is_empty() {
            return Err(Error::invalid("content", "a revision needs at least one file"));
        }
        let source = match map.get("source") {
            None => None,
            Some(Value::Map(m)) => Some(m.clone()),
            Some(_) => return Err(Error::invalid("source", "must be a map")),
        };
        Ok(Revision {
            item,
            parents,
            author: f.key("author")?,
            clock: f.clock()?,
            time: f.time()?,
            kind: Kind::parse(&f.text("kind")?)?,
            title: f.bounded_text("title", MAX_TITLE)?,
            lang: lang(&f.text("lang")?)?,
            topics: f.topics("topics")?,
            license: license(&f.text("license")?)?,
            vis,
            content,
            source,
            summary: f.opt_bounded_text("summary", MAX_NOTE)?,
            extra: map.without(REVISION_FIELDS),
        })
    }

    fn signer(&self) -> Option<PublicKey> {
        Some(self.author)
    }
}

// ---------------------------------------------------------------------------------------------
// Attestation

/// What an attestation is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Object(Id),
    Key(PublicKey),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    Verified,
    Dangerous,
    Outdated,
    Hide,
    TranslationOf,
    DuplicateOf,
    Replaces,
    Trust,
    Withdraw,
    /// A claim from a newer format version.
    Other(String),
}

impl Claim {
    pub fn as_str(&self) -> &str {
        match self {
            Claim::Verified => "verified",
            Claim::Dangerous => "dangerous",
            Claim::Outdated => "outdated",
            Claim::Hide => "hide",
            Claim::TranslationOf => "translation-of",
            Claim::DuplicateOf => "duplicate-of",
            Claim::Replaces => "replaces",
            Claim::Trust => "trust",
            Claim::Withdraw => "withdraw",
            Claim::Other(s) => s,
        }
    }

    fn parse(s: &str) -> Result<Claim> {
        Ok(match s {
            "verified" => Claim::Verified,
            "dangerous" => Claim::Dangerous,
            "outdated" => Claim::Outdated,
            "hide" => Claim::Hide,
            "translation-of" => Claim::TranslationOf,
            "duplicate-of" => Claim::DuplicateOf,
            "replaces" => Claim::Replaces,
            "trust" => Claim::Trust,
            "withdraw" => Claim::Withdraw,
            other => Claim::Other(word("claim", other)?),
        })
    }
}

/// A signed statement about a revision, an item or a key (spec section 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
    pub about: Subject,
    pub claim: Claim,
    pub topics: Vec<Topic>,
    pub related: Option<Id>,
    pub note: Option<String>,
    pub author: PublicKey,
    pub clock: u64,
    pub time: Time,
    pub extra: Map,
}

const ATTESTATION_FIELDS: &[&str] = &[
    "t", "v", "sig", "about", "claim", "topics", "related", "note", "author", "clock", "time",
];

impl Object for Attestation {
    const TYPE: &'static str = "att";

    fn to_map(&self) -> Map {
        let mut m = self.extra.clone();
        let about: Value = match self.about {
            Subject::Object(id) => id.into(),
            Subject::Key(key) => key.into(),
        };
        m.insert("about", about)
            .insert("claim", self.claim.as_str())
            .insert("author", self.author)
            .insert("clock", self.clock)
            .insert("time", self.time.to_value())
            .insert_opt("related", self.related)
            .insert_opt("note", self.note.as_deref());
        if !self.topics.is_empty() {
            m.insert("topics", topics_value(&self.topics));
        }
        m
    }

    fn from_map(map: &Map) -> Result<Attestation> {
        let f = Fields(map);
        let about = match map.get("about") {
            Some(Value::Bytes(b)) if b.len() == 32 => Subject::Key(PublicKey::from_bytes(b)?),
            Some(Value::Bytes(b)) => Subject::Object(Id::from_multihash(b)?),
            Some(_) => return Err(Error::invalid("about", "must be an ID or a key")),
            None => return Err(Error::Missing("about")),
        };
        let claim = Claim::parse(&f.text("claim")?)?;
        let topics = if map.contains("topics") {
            f.topics("topics")?
        } else {
            Vec::new()
        };
        let related = f.opt_id("related")?;

        let needs_object = !matches!(claim, Claim::Trust | Claim::Other(_));
        if needs_object && !matches!(about, Subject::Object(_)) {
            return Err(Error::invalid(
                "about",
                format!("`{}` must be about an object", claim.as_str()),
            ));
        }
        if claim == Claim::Trust && !matches!(about, Subject::Key(_)) {
            return Err(Error::invalid("about", "`trust` must name a key"));
        }
        if matches!(claim, Claim::Trust | Claim::Verified) && topics.is_empty() {
            return Err(Error::invalid(
                "topics",
                format!("`{}` needs at least one topic", claim.as_str()),
            ));
        }
        let needs_related = matches!(claim, Claim::TranslationOf | Claim::DuplicateOf | Claim::Replaces);
        if needs_related && related.is_none() {
            return Err(Error::Missing("related"));
        }

        Ok(Attestation {
            about,
            claim,
            topics,
            related,
            note: f.opt_bounded_text("note", MAX_NOTE)?,
            author: f.key("author")?,
            clock: f.clock()?,
            time: f.time()?,
            extra: map.without(ATTESTATION_FIELDS),
        })
    }

    fn signer(&self) -> Option<PublicKey> {
        Some(self.author)
    }
}

// ---------------------------------------------------------------------------------------------
// Keys

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Person,
    Node,
    Catalog,
    /// A kind added by a later spec version. Stored and passed on, never trusted by default.
    Other(String),
}

impl KeyKind {
    pub fn as_str(&self) -> &str {
        match self {
            KeyKind::Person => "person",
            KeyKind::Node => "node",
            KeyKind::Catalog => "catalog",
            KeyKind::Other(s) => s,
        }
    }
}

/// Who a key belongs to. Signed by that key (spec section 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRecord {
    pub key: PublicKey,
    pub kind: KeyKind,
    pub name: String,
    /// A second key, kept offline, that can revoke this one.
    pub recovery: Option<PublicKey>,
    pub clock: u64,
    pub time: Time,
    pub extra: Map,
}

const KEY_FIELDS: &[&str] = &["t", "v", "sig", "key", "kind", "name", "recovery", "clock", "time"];

impl Object for KeyRecord {
    const TYPE: &'static str = "key";

    fn to_map(&self) -> Map {
        let mut m = self.extra.clone();
        m.insert("key", self.key)
            .insert("kind", self.kind.as_str())
            .insert("name", self.name.as_str())
            .insert_opt("recovery", self.recovery)
            .insert("clock", self.clock)
            .insert("time", self.time.to_value());
        m
    }

    fn from_map(map: &Map) -> Result<KeyRecord> {
        let f = Fields(map);
        let key = f.key("key")?;
        let kind = match f.text("kind")?.as_str() {
            "person" => KeyKind::Person,
            "node" => KeyKind::Node,
            "catalog" => KeyKind::Catalog,
            other => KeyKind::Other(word("kind", other)?),
        };
        let recovery = f.opt_key("recovery")?;
        if recovery == Some(key) {
            return Err(Error::invalid("recovery", "must differ from the key itself"));
        }
        Ok(KeyRecord {
            key,
            kind,
            name: f.bounded_text("name", 200)?,
            recovery,
            clock: f.clock()?,
            time: f.time()?,
            extra: map.without(KEY_FIELDS),
        })
    }

    fn signer(&self) -> Option<PublicKey> {
        Some(self.key)
    }
}

/// Withdraws a key from a clock value on. Signed by the key itself or its recovery key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revoke {
    pub key: PublicKey,
    /// Objects the key signed with this clock value or higher are no longer valid.
    pub from: u64,
    /// The signer: `key` itself, or the recovery key from its key record.
    pub by: PublicKey,
    pub clock: u64,
    pub time: Time,
    pub extra: Map,
}

const REVOKE_FIELDS: &[&str] = &["t", "v", "sig", "key", "from", "by", "clock", "time"];

impl Revoke {
    /// Whether `by` may revoke `key`, given every key record seen for that key.
    ///
    /// A recovery key named in *any* record counts. Picking one record (the latest, say) would
    /// let a thief who stole the main key publish a new record naming their own recovery key and
    /// so block the owner's revocation. Accepting all costs nothing: the thief can already revoke
    /// the key with the key itself.
    pub fn is_authorized(&self, records: &[KeyRecord]) -> bool {
        self.by == self.key || records.iter().any(|r| r.key == self.key && r.recovery == Some(self.by))
    }
}

impl Object for Revoke {
    const TYPE: &'static str = "revoke";

    fn to_map(&self) -> Map {
        let mut m = self.extra.clone();
        m.insert("key", self.key)
            .insert("from", self.from)
            .insert("by", self.by)
            .insert("clock", self.clock)
            .insert("time", self.time.to_value());
        m
    }

    fn from_map(map: &Map) -> Result<Revoke> {
        let f = Fields(map);
        Ok(Revoke {
            key: f.key("key")?,
            from: f.uint("from")?,
            by: f.key("by")?,
            clock: f.clock()?,
            time: f.time()?,
            extra: map.without(REVOKE_FIELDS),
        })
    }

    fn signer(&self) -> Option<PublicKey> {
        Some(self.by)
    }
}

// ---------------------------------------------------------------------------------------------
// Derived data

/// A result computed from a blob: text, transcript, embeddings, thumbnail, index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived {
    pub from: Id,
    /// Tool name and version, e.g. `nevo-ocr/1.2`.
    pub producer: String,
    pub out: Id,
    pub author: PublicKey,
    pub extra: Map,
}

const DERIVED_FIELDS: &[&str] = &["t", "v", "sig", "from", "producer", "out", "author"];

impl Object for Derived {
    const TYPE: &'static str = "derived";

    fn to_map(&self) -> Map {
        let mut m = self.extra.clone();
        m.insert("from", self.from)
            .insert("producer", self.producer.as_str())
            .insert("out", self.out)
            .insert("author", self.author);
        m
    }

    fn from_map(map: &Map) -> Result<Derived> {
        let f = Fields(map);
        let producer = f.bounded_text("producer", 200)?;
        if !producer.contains('/') {
            return Err(Error::invalid("producer", "must be `name/version`"));
        }
        Ok(Derived {
            from: f.id("from")?,
            producer,
            out: f.id("out")?,
            author: f.key("author")?,
            extra: map.without(DERIVED_FIELDS),
        })
    }

    fn signer(&self) -> Option<PublicKey> {
        Some(self.author)
    }
}

// ---------------------------------------------------------------------------------------------
// Logs (spec section 4)

/// One entry of a node's append-only log. Unsigned: the log head vouches for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub seq: u64,
    /// ID of the previous entry; `None` only for entry 1.
    pub prev: Option<Id>,
    pub obj: Id,
    pub extra: Map,
}

const LOG_ENTRY_FIELDS: &[&str] = &["t", "v", "seq", "prev", "obj"];

impl Object for LogEntry {
    const TYPE: &'static str = "log";

    fn to_map(&self) -> Map {
        let mut m = self.extra.clone();
        m.insert("seq", self.seq)
            .insert_opt("prev", self.prev)
            .insert("obj", self.obj);
        m
    }

    fn from_map(map: &Map) -> Result<LogEntry> {
        let f = Fields(map);
        let seq = f.uint("seq")?;
        let prev = f.opt_id("prev")?;
        if seq == 0 || (seq == 1) != prev.is_none() {
            return Err(Error::invalid(
                "seq",
                "starts at 1; every later entry names the previous one",
            ));
        }
        Ok(LogEntry {
            seq,
            prev,
            obj: f.id("obj")?,
            extra: map.without(LOG_ENTRY_FIELDS),
        })
    }

    fn signer(&self) -> Option<PublicKey> {
        None
    }
}

/// A node's signature over its log up to `seq`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogHead {
    pub node: PublicKey,
    pub seq: u64,
    /// ID of the entry with this `seq`.
    pub entry: Id,
    pub extra: Map,
}

const LOG_HEAD_FIELDS: &[&str] = &["t", "v", "sig", "node", "seq", "entry"];

impl Object for LogHead {
    const TYPE: &'static str = "head";

    fn to_map(&self) -> Map {
        let mut m = self.extra.clone();
        m.insert("node", self.node)
            .insert("seq", self.seq)
            .insert("entry", self.entry);
        m
    }

    fn from_map(map: &Map) -> Result<LogHead> {
        let f = Fields(map);
        let seq = f.uint("seq")?;
        if seq == 0 {
            return Err(Error::invalid("seq", "starts at 1"));
        }
        Ok(LogHead {
            node: f.key("node")?,
            seq,
            entry: f.id("entry")?,
            extra: map.without(LOG_HEAD_FIELDS),
        })
    }

    fn signer(&self) -> Option<PublicKey> {
        Some(self.node)
    }
}

// ---------------------------------------------------------------------------------------------
// Field helpers

struct Fields<'a>(&'a Map);

impl Fields<'_> {
    fn req(&self, k: &'static str) -> Result<&Value> {
        self.0.get(k).ok_or(Error::Missing(k))
    }

    fn text(&self, k: &'static str) -> Result<String> {
        match self.req(k)? {
            Value::Text(s) => Ok(s.clone()),
            _ => Err(Error::invalid(k, "must be text")),
        }
    }

    fn bounded_text(&self, k: &'static str, max_chars: usize) -> Result<String> {
        let s = self.text(k)?;
        if s.trim().is_empty() || s.chars().count() > max_chars {
            return Err(Error::invalid(k, format!("must be 1 to {max_chars} characters")));
        }
        Ok(s)
    }

    fn opt_bounded_text(&self, k: &'static str, max_chars: usize) -> Result<Option<String>> {
        self.0.contains(k).then(|| self.bounded_text(k, max_chars)).transpose()
    }

    fn uint(&self, k: &'static str) -> Result<u64> {
        match self.req(k)? {
            // Capped at 2^63 - 1 so every value fits a signed 64-bit database integer.
            Value::Int(n) if (0..=i128::from(i64::MAX)).contains(n) => Ok(*n as u64),
            Value::Int(_) => Err(Error::invalid(k, "must be 0 to 2^63 - 1")),
            _ => Err(Error::invalid(k, "must be an integer")),
        }
    }

    fn int(&self, k: &'static str) -> Result<i64> {
        match self.req(k)? {
            Value::Int(n) => i64::try_from(*n).map_err(|_| Error::invalid(k, "out of range")),
            _ => Err(Error::invalid(k, "must be an integer")),
        }
    }

    fn clock(&self) -> Result<u64> {
        match self.uint("clock")? {
            0 => Err(Error::invalid("clock", "starts at 1")),
            n => Ok(n),
        }
    }

    fn time(&self) -> Result<Time> {
        Time::from_value(self.req("time")?)
    }

    fn id(&self, k: &'static str) -> Result<Id> {
        match self.req(k)? {
            Value::Bytes(b) => Id::from_multihash(b),
            _ => Err(Error::invalid(k, "must be an ID")),
        }
    }

    fn opt_id(&self, k: &'static str) -> Result<Option<Id>> {
        self.0.contains(k).then(|| self.id(k)).transpose()
    }

    fn key(&self, k: &'static str) -> Result<PublicKey> {
        match self.req(k)? {
            Value::Bytes(b) => PublicKey::from_bytes(b),
            _ => Err(Error::invalid(k, "must be a public key")),
        }
    }

    fn opt_key(&self, k: &'static str) -> Result<Option<PublicKey>> {
        self.0.contains(k).then(|| self.key(k)).transpose()
    }

    fn array(&self, k: &'static str) -> Result<&Vec<Value>> {
        match self.req(k)? {
            Value::Array(items) if items.len() <= MAX_LIST => Ok(items),
            Value::Array(_) => Err(Error::invalid(k, format!("at most {MAX_LIST} entries"))),
            _ => Err(Error::invalid(k, "must be an array")),
        }
    }

    fn ids(&self, k: &'static str) -> Result<Vec<Id>> {
        let ids = self
            .array(k)?
            .iter()
            .map(|v| match v {
                Value::Bytes(b) => Id::from_multihash(b),
                _ => Err(Error::invalid(k, "entries must be IDs")),
            })
            .collect::<Result<Vec<_>>>()?;
        if has_duplicates(&ids) {
            return Err(Error::invalid(k, "repeated entry"));
        }
        Ok(ids)
    }

    fn topics(&self, k: &'static str) -> Result<Vec<Topic>> {
        let topics = self
            .array(k)?
            .iter()
            .map(|v| match v {
                Value::Text(s) => s.parse::<Topic>(),
                _ => Err(Error::invalid(k, "entries must be text")),
            })
            .collect::<Result<Vec<_>>>()?;
        if topics.is_empty() || topics.len() > 16 {
            return Err(Error::invalid(k, "1 to 16 topics"));
        }
        if has_duplicates(&topics) {
            return Err(Error::invalid(k, "repeated topic"));
        }
        Ok(topics)
    }
}

fn has_duplicates<T: Ord + Clone>(items: &[T]) -> bool {
    let mut sorted = items.to_vec();
    sorted.sort();
    sorted.windows(2).any(|w| w[0] == w[1])
}

fn ids_value(ids: &[Id]) -> Value {
    Value::Array(ids.iter().map(|&id| id.into()).collect())
}

fn topics_value(topics: &[Topic]) -> Value {
    Value::Array(topics.iter().map(|t| t.as_str().into()).collect())
}

/// A lowercase word for open-ended enums (kinds and claims from newer versions).
fn word(field: &'static str, s: &str) -> Result<String> {
    let ok = !s.is_empty()
        && s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(s.to_string())
    } else {
        Err(Error::invalid(field, format!("unknown value `{s}`")))
    }
}

fn lang(s: &str) -> Result<String> {
    let ok = (2..=35).contains(&s.len())
        && s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && s.split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric()));
    if ok {
        Ok(s.to_string())
    } else {
        Err(Error::invalid("lang", format!("`{s}` is not a BCP 47 tag")))
    }
}

fn license(s: &str) -> Result<String> {
    let ok = !s.is_empty() && s.len() <= 200 && s.bytes().all(|b| b.is_ascii_graphic() || b == b' ');
    if ok {
        Ok(s.to_string())
    } else {
        Err(Error::invalid("license", format!("`{s}` is not an SPDX expression")))
    }
}
