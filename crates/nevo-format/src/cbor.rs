//! Deterministic CBOR (RFC 8949 §4.2.1), limited to the types the format allows.
//!
//! The decoder is strict: it accepts only the one canonical encoding of each value, so bytes that
//! decode successfully always re-encode to themselves. That is what makes an object's ID (the hash
//! of its bytes) unambiguous.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use unicode_normalization::{is_nfc, UnicodeNormalization};

use crate::error::{Error, Result};

/// Deepest nesting the decoder accepts. Real objects use about 4 levels.
pub const MAX_DEPTH: usize = 16;

/// Largest integer magnitude CBOR can carry: integers range over -2^64 ..= 2^64-1.
const INT_MIN: i128 = -(1 << 64);
const INT_MAX: i128 = (1 << 64) - 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i128),
    Bytes(Vec<u8>),
    /// Text is always NFC-normalized when encoded.
    Text(String),
    Array(Vec<Value>),
    Map(Map),
    Bool(bool),
    Null,
}

/// A map key, ordered the way deterministic CBOR orders encoded text keys: shorter first, then
/// bytewise. (The encoded length prefix grows with the length, so this matches comparing the
/// encoded bytes.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key(pub String);

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.as_bytes().cmp(other.0.as_bytes()))
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A CBOR map with text keys, kept in canonical order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Map(BTreeMap<Key, Value>);

impl Map {
    pub fn new() -> Map {
        Map::default()
    }

    pub fn insert(&mut self, key: &str, value: impl Into<Value>) -> &mut Map {
        self.0.insert(Key(key.to_string()), value.into());
        self
    }

    /// Inserts only when there is a value.
    pub fn insert_opt<V: Into<Value>>(&mut self, key: &str, value: Option<V>) -> &mut Map {
        if let Some(v) = value {
            self.insert(key, v);
        }
        self
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(&Key(key.to_string()))
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.0.remove(&Key(key.to_string()))
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.0.as_str(), v))
    }

    /// The entries whose keys are not in `known`: fields from newer format versions, kept so
    /// that re-encoding an object never changes its bytes.
    pub fn without(&self, known: &[&str]) -> Map {
        Map(self
            .0
            .iter()
            .filter(|(k, _)| !known.contains(&k.0.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }

    /// Adds every entry of `other`, keeping existing ones on a clash.
    pub fn extend_missing(&mut self, other: &Map) {
        for (k, v) in &other.0 {
            self.0.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

impl From<i128> for Value {
    fn from(v: i128) -> Value {
        Value::Int(v)
    }
}
impl From<u64> for Value {
    fn from(v: u64) -> Value {
        Value::Int(v.into())
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Value {
        Value::Int(v.into())
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Value {
        Value::Int(v.into())
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Text(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Text(v)
    }
}
impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Value {
        Value::Bytes(v)
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Value {
        Value::Array(v)
    }
}
impl From<Map> for Value {
    fn from(v: Map) -> Value {
        Value::Map(v)
    }
}

pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_value(&mut out, value);
    out
}

pub fn encode_map(map: &Map) -> Vec<u8> {
    let mut out = Vec::new();
    write_map(&mut out, map);
    out
}

fn write_head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | n as u8),
        24..=0xff => out.extend_from_slice(&[m | 24, n as u8]),
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(m | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

fn write_text(out: &mut Vec<u8>, s: &str) {
    if is_nfc(s) {
        write_head(out, 3, s.len() as u64);
        out.extend_from_slice(s.as_bytes());
    } else {
        let normalized: String = s.nfc().collect();
        write_head(out, 3, normalized.len() as u64);
        out.extend_from_slice(normalized.as_bytes());
    }
}

fn write_map(out: &mut Vec<u8>, map: &Map) {
    // Normalizing a key can change its order, so sort the encoded keys rather than trusting
    // the in-memory order (which is already right for NFC keys, the normal case).
    let mut entries: Vec<(Vec<u8>, &Value)> = map
        .0
        .iter()
        .map(|(k, v)| {
            let mut key = Vec::new();
            write_text(&mut key, &k.0);
            (key, v)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup_by(|a, b| a.0 == b.0);
    write_head(out, 5, entries.len() as u64);
    for (key, value) in entries {
        out.extend_from_slice(&key);
        write_value(out, value);
    }
}

fn write_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Int(n) => {
            assert!((INT_MIN..=INT_MAX).contains(n), "integer out of CBOR range: {n}");
            if *n >= 0 {
                write_head(out, 0, *n as u64);
            } else {
                write_head(out, 1, (-1 - *n) as u64);
            }
        }
        Value::Bytes(b) => {
            write_head(out, 2, b.len() as u64);
            out.extend_from_slice(b);
        }
        Value::Text(s) => write_text(out, s),
        Value::Array(items) => {
            write_head(out, 4, items.len() as u64);
            for item in items {
                write_value(out, item);
            }
        }
        Value::Map(map) => write_map(out, map),
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Null => out.push(0xf6),
    }
}

/// Decodes exactly one canonical value that fills `bytes`.
pub fn decode(bytes: &[u8]) -> Result<Value> {
    let mut d = Decoder { buf: bytes, pos: 0 };
    let value = d.value(0)?;
    if d.pos != bytes.len() {
        return Err(Error::Cbor("trailing bytes"));
    }
    Ok(value)
}

struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Decoder<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.buf.len() - self.pos < n {
            return Err(Error::Cbor("unexpected end"));
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Reads a head and returns (major type, argument). Rejects non-shortest arguments.
    fn head(&mut self) -> Result<(u8, u8, u64)> {
        let first = self.take(1)?[0];
        let (major, info) = (first >> 5, first & 31);
        if major == 7 {
            return Ok((major, info, 0));
        }
        let n = match info {
            0..=23 => u64::from(info),
            24 => u64::from(self.take(1)?[0]),
            25 => u64::from(u16::from_be_bytes(self.take(2)?.try_into().unwrap())),
            26 => u64::from(u32::from_be_bytes(self.take(4)?.try_into().unwrap())),
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            31 => return Err(Error::Cbor("indefinite lengths are not allowed")),
            _ => return Err(Error::Cbor("reserved additional information")),
        };
        let shortest = match info {
            24 => n >= 24,
            25 => n > 0xff,
            26 => n > 0xffff,
            27 => n > 0xffff_ffff,
            _ => true,
        };
        if !shortest {
            return Err(Error::Cbor("integer or length not in shortest form"));
        }
        Ok((major, info, n))
    }

    fn length(&self, n: u64) -> Result<usize> {
        usize::try_from(n)
            .ok()
            .filter(|&n| n <= self.remaining())
            .ok_or(Error::Cbor("length exceeds input"))
    }

    fn text(&mut self, n: u64) -> Result<String> {
        let len = self.length(n)?;
        let s = std::str::from_utf8(self.take(len)?).map_err(|_| Error::Cbor("text is not UTF-8"))?;
        if !is_nfc(s) {
            return Err(Error::Cbor("text is not NFC-normalized"));
        }
        Ok(s.to_string())
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(Error::Cbor("nested too deeply"));
        }
        let (major, info, n) = self.head()?;
        Ok(match major {
            0 => Value::Int(i128::from(n)),
            1 => Value::Int(-1 - i128::from(n)),
            2 => {
                let len = self.length(n)?;
                Value::Bytes(self.take(len)?.to_vec())
            }
            3 => Value::Text(self.text(n)?),
            4 => {
                // Every item takes at least one byte, so `count` is bounded by the input, but not
                // across nesting levels: each level could reserve room for the whole input. Reserve
                // a little and let the vector grow as items actually decode.
                let count = self.length(n)?;
                let mut items = Vec::with_capacity(count.min(64));
                for _ in 0..count {
                    items.push(self.value(depth + 1)?);
                }
                Value::Array(items)
            }
            5 => {
                let count = self.length(n)?;
                let mut map = BTreeMap::new();
                let mut previous: Option<Key> = None;
                for _ in 0..count {
                    let (kmajor, _, klen) = self.head()?;
                    if kmajor != 3 {
                        return Err(Error::Cbor("map keys must be text"));
                    }
                    let key = Key(self.text(klen)?);
                    if previous.as_ref().is_some_and(|p| *p >= key) {
                        return Err(Error::Cbor("map keys out of order or repeated"));
                    }
                    let value = self.value(depth + 1)?;
                    previous = Some(key.clone());
                    map.insert(key, value);
                }
                Value::Map(Map(map))
            }
            6 => return Err(Error::Cbor("tags are not allowed")),
            _ => match info {
                20 => Value::Bool(false),
                21 => Value::Bool(true),
                22 => Value::Null,
                25..=27 => return Err(Error::Cbor("floats are not allowed")),
                _ => return Err(Error::Cbor("simple value not allowed")),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn rfc8949_appendix_a_vectors() {
        let cases: Vec<(Value, &str)> = vec![
            (0u64.into(), "00"),
            (23u64.into(), "17"),
            (24u64.into(), "1818"),
            (1000u64.into(), "1903e8"),
            (1_000_000u64.into(), "1a000f4240"),
            (u64::MAX.into(), "1bffffffffffffffff"),
            (Value::Int(-1), "20"),
            (Value::Int(-1000), "3903e7"),
            (Value::Int(INT_MIN), "3bffffffffffffffff"),
            ("".into(), "60"),
            ("IETF".into(), "6449455446"),
            ("\u{00fc}".into(), "62c3bc"),
            (Value::Bytes(vec![1, 2, 3, 4]), "4401020304"),
            (Value::Array(vec![1u64.into(), 2u64.into(), 3u64.into()]), "83010203"),
            (Value::Bool(false), "f4"),
            (Value::Bool(true), "f5"),
            (Value::Null, "f6"),
        ];
        for (value, coded) in cases {
            assert_eq!(encode(&value), hex(coded), "{value:?}");
            assert_eq!(decode(&hex(coded)).unwrap(), value);
        }
    }

    #[test]
    fn map_keys_sort_shorter_first() {
        let mut m = Map::new();
        m.insert("bb", 1u64).insert("a", 2u64).insert("c", 3u64);
        // {"a": 2, "c": 3, "bb": 1}
        assert_eq!(encode_map(&m), hex("a3616102616303626262 01".replace(' ', "").as_str()));
    }

    #[test]
    fn text_is_normalized_when_encoded() {
        let composed = encode(&"caf\u{e9}".into());
        let decomposed = encode(&"cafe\u{301}".into());
        assert_eq!(composed, decomposed);
    }

    #[test]
    fn rejects_non_canonical_input() {
        for (bytes, why) in [
            ("1817", "24 with a one-byte argument"),
            ("190017", "23 with a two-byte argument"),
            ("5f4101ff", "indefinite byte string"),
            ("c100", "tag"),
            ("f93c00", "half float"),
            ("f7", "undefined"),
            ("a2616201616101", "keys out of order"),
            ("a2616101616102", "repeated key"),
            ("a10101", "integer key"),
            ("6663616665cc81", "text not NFC (cafe + combining accent)"),
            ("0000", "trailing bytes"),
            ("62c3", "truncated"),
            ("9bffffffffffffffff", "huge array length"),
        ] {
            assert!(decode(&hex(bytes)).is_err(), "should reject {why}");
        }
    }

    #[test]
    fn rejects_deep_nesting() {
        let bytes = vec![0x81; MAX_DEPTH + 2].into_iter().chain([0x00]).collect::<Vec<_>>();
        assert!(decode(&bytes).is_err());
    }
}
