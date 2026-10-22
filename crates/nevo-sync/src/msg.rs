//! Protocol messages. Each is a deterministic CBOR map (the same encoding as objects), so
//! objects, entries and blob pieces travel as byte strings with no re-encoding and no base64.
//! Unknown keys are ignored, so a later protocol version can add fields.

use nevo_format::cbor::{self, Map, Value};
use nevo_format::{Id, PublicKey};

use crate::error::{Result, SyncError};

/// A message that travels as one CBOR map.
pub trait Message: Sized {
    fn to_map(&self) -> Map;
    fn from_map(map: &Map) -> Result<Self>;

    fn encode(&self) -> Vec<u8> {
        cbor::encode_map(&self.to_map())
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        match cbor::decode(bytes)? {
            Value::Map(map) => Self::from_map(&map),
            _ => Err(bad("message is not a map")),
        }
    }
}

/// `GET /hello` answer. Not signed: it only says who the server claims to be, and every later
/// answer is signed by that key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Protocol versions the server speaks.
    pub versions: Vec<u64>,
    pub node: PublicKey,
    /// Random bytes the client signs into its requests, so a recorded request can't be replayed.
    pub challenge: Vec<u8>,
}

/// `heads`: the highest signed head the server can serve for each log it has, a page at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadsRequest {
    /// Continue after this node (the `next` of the previous page).
    pub after: Option<PublicKey>,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadsResponse {
    /// Encoded log heads, one per node.
    pub heads: Vec<Vec<u8>>,
    /// Encoded heads that prove a node forked its log, for nodes on this page.
    pub forks: Vec<Vec<u8>>,
    /// Where the next page starts, if there is one.
    pub next: Option<PublicKey>,
}

/// `entries`: entries of one log after `after`, up to `upto` at most.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntriesRequest {
    pub node: PublicKey,
    pub after: u64,
    pub upto: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntriesResponse {
    pub entries: Vec<Vec<u8>>,
    /// The signed head the entries end at, when one exists in the range sent. Without one, the
    /// client keeps the entries until a later answer reaches a head.
    pub head: Option<Vec<u8>>,
}

/// `objects`: objects by ID. The server sends only what it may share and has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectsRequest {
    pub ids: Vec<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectsResponse {
    pub objects: Vec<Vec<u8>>,
    /// How many of the requested IDs were looked at. Fewer than asked means the answer hit its
    /// size cap; the client asks again for the rest.
    pub done: u64,
}

/// `blob`: part of a blob, starting at a piece boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRequest {
    pub id: Id,
    pub from: u64,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobResponse {
    /// The whole blob's size.
    pub size: u64,
    pub data: Vec<u8>,
}

impl Message for Hello {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        let versions = self.versions.iter().map(|&v| Value::from(v)).collect::<Vec<_>>();
        m.insert("versions", versions)
            .insert("node", self.node)
            .insert("challenge", self.challenge.clone());
        m
    }

    fn from_map(m: &Map) -> Result<Hello> {
        let versions = match m.get("versions") {
            Some(Value::Array(a)) => a.iter().map(as_u64).collect::<Result<Vec<_>>>()?,
            _ => return Err(bad("versions")),
        };
        Ok(Hello {
            versions,
            node: key(m, "node")?,
            challenge: bytes(m, "challenge")?.to_vec(),
        })
    }
}

impl Message for HeadsRequest {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert_opt("after", self.after).insert("limit", self.limit);
        m
    }

    fn from_map(m: &Map) -> Result<HeadsRequest> {
        Ok(HeadsRequest {
            after: opt_key(m, "after")?,
            limit: uint(m, "limit")?,
        })
    }
}

impl Message for HeadsResponse {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert("heads", byte_list(&self.heads))
            .insert("forks", byte_list(&self.forks))
            .insert_opt("next", self.next);
        m
    }

    fn from_map(m: &Map) -> Result<HeadsResponse> {
        Ok(HeadsResponse {
            heads: list_of_bytes(m, "heads")?,
            forks: list_of_bytes(m, "forks")?,
            next: opt_key(m, "next")?,
        })
    }
}

impl Message for EntriesRequest {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert("node", self.node)
            .insert("after", self.after)
            .insert("upto", self.upto);
        m
    }

    fn from_map(m: &Map) -> Result<EntriesRequest> {
        Ok(EntriesRequest {
            node: key(m, "node")?,
            after: uint(m, "after")?,
            upto: uint(m, "upto")?,
        })
    }
}

impl Message for EntriesResponse {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert("entries", byte_list(&self.entries))
            .insert_opt("head", self.head.clone());
        m
    }

    fn from_map(m: &Map) -> Result<EntriesResponse> {
        Ok(EntriesResponse {
            entries: list_of_bytes(m, "entries")?,
            head: match m.get("head") {
                None | Some(Value::Null) => None,
                Some(_) => Some(bytes(m, "head")?.to_vec()),
            },
        })
    }
}

impl Message for ObjectsRequest {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert("ids", self.ids.iter().map(|&id| Value::from(id)).collect::<Vec<_>>());
        m
    }

    fn from_map(m: &Map) -> Result<ObjectsRequest> {
        let ids = list_of_bytes(m, "ids")?
            .iter()
            .map(|b| Id::from_multihash(b).map_err(SyncError::from))
            .collect::<Result<Vec<_>>>()?;
        Ok(ObjectsRequest { ids })
    }
}

impl Message for ObjectsResponse {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert("objects", byte_list(&self.objects)).insert("done", self.done);
        m
    }

    fn from_map(m: &Map) -> Result<ObjectsResponse> {
        Ok(ObjectsResponse {
            objects: list_of_bytes(m, "objects")?,
            done: uint(m, "done")?,
        })
    }
}

impl Message for BlobRequest {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert("id", self.id)
            .insert("from", self.from)
            .insert("len", self.len);
        m
    }

    fn from_map(m: &Map) -> Result<BlobRequest> {
        Ok(BlobRequest {
            id: Id::from_multihash(bytes(m, "id")?)?,
            from: uint(m, "from")?,
            len: uint(m, "len")?,
        })
    }
}

impl Message for BlobResponse {
    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert("size", self.size).insert("data", self.data.clone());
        m
    }

    fn from_map(m: &Map) -> Result<BlobResponse> {
        Ok(BlobResponse {
            size: uint(m, "size")?,
            data: bytes(m, "data")?.to_vec(),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Field helpers

fn bad(what: &str) -> SyncError {
    SyncError::Protocol(format!("bad or missing field in message: {what}"))
}

fn as_u64(v: &Value) -> Result<u64> {
    match v {
        Value::Int(i) => u64::try_from(*i).map_err(|_| bad("integer out of range")),
        _ => Err(bad("expected an integer")),
    }
}

fn uint(m: &Map, field: &str) -> Result<u64> {
    m.get(field).ok_or_else(|| bad(field)).and_then(as_u64)
}

fn bytes<'a>(m: &'a Map, field: &str) -> Result<&'a [u8]> {
    match m.get(field) {
        Some(Value::Bytes(b)) => Ok(b),
        _ => Err(bad(field)),
    }
}

fn key(m: &Map, field: &str) -> Result<PublicKey> {
    Ok(PublicKey::from_bytes(bytes(m, field)?)?)
}

fn opt_key(m: &Map, field: &str) -> Result<Option<PublicKey>> {
    match m.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => key(m, field).map(Some),
    }
}

fn byte_list(items: &[Vec<u8>]) -> Vec<Value> {
    items.iter().map(|b| Value::Bytes(b.clone())).collect()
}

fn list_of_bytes(m: &Map, field: &str) -> Result<Vec<Vec<u8>>> {
    match m.get(field) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::Bytes(b) => Ok(b.clone()),
                _ => Err(bad(field)),
            })
            .collect(),
        _ => Err(bad(field)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nevo_format::Keypair;

    fn round_trip<M: Message + PartialEq + std::fmt::Debug>(m: M) {
        assert_eq!(M::decode(&m.encode()).unwrap(), m);
    }

    #[test]
    fn messages_round_trip() {
        let k = Keypair::from_secret([3; 32]).public();
        round_trip(Hello {
            versions: vec![1, 2],
            node: k,
            challenge: vec![9; 16],
        });
        round_trip(HeadsRequest { after: None, limit: 5 });
        round_trip(HeadsRequest {
            after: Some(k),
            limit: 5,
        });
        round_trip(HeadsResponse {
            heads: vec![vec![1, 2]],
            forks: vec![],
            next: Some(k),
        });
        round_trip(EntriesRequest {
            node: k,
            after: 3,
            upto: 9,
        });
        round_trip(EntriesResponse {
            entries: vec![vec![1], vec![2]],
            head: Some(vec![3]),
        });
        round_trip(EntriesResponse {
            entries: vec![],
            head: None,
        });
        round_trip(ObjectsRequest {
            ids: vec![Id::of(b"a"), Id::of(b"b")],
        });
        round_trip(ObjectsResponse {
            objects: vec![vec![7; 10]],
            done: 2,
        });
        round_trip(BlobRequest {
            id: Id::of(b"x"),
            from: 1 << 20,
            len: 1 << 20,
        });
        round_trip(BlobResponse {
            size: 3,
            data: vec![1, 2, 3],
        });
    }

    #[test]
    fn unknown_fields_are_ignored_and_bad_ones_refused() {
        let mut m = HeadsRequest { after: None, limit: 5 }.to_map();
        m.insert("future", "field");
        assert_eq!(
            HeadsRequest::decode(&cbor::encode_map(&m)).unwrap(),
            HeadsRequest { after: None, limit: 5 }
        );
        m.insert("limit", "five");
        assert!(HeadsRequest::decode(&cbor::encode_map(&m)).is_err());
        assert!(HeadsRequest::decode(b"\x01").is_err());
    }
}
