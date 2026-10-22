"""Encoding check for docs/spec/content-model.md (draft 0.1).

Builds realistic objects, then checks that:
- the encoding is deterministic (key order in the source doesn't change the bytes or the ID)
- signatures verify, and fail after any change
- objects are small enough for a Pi-sized library

Throwaway spike code. Needs: cbor2, blake3, pynacl.
  python encode_test.py
"""

import random
import unicodedata

import blake3
import cbor2
from nacl.signing import SigningKey

MULTIHASH_BLAKE3 = bytes([0x1E, 0x20])


def b32(data: bytes) -> str:
    import base64
    return base64.b32encode(data).decode().lower().rstrip("=")


def object_id(encoded: bytes) -> bytes:
    return MULTIHASH_BLAKE3 + blake3.blake3(encoded).digest()


def id_text(oid: bytes) -> str:
    return "b" + b32(oid)


def nfc(value):
    """NFC-normalize every string, as the spec requires."""
    if isinstance(value, str):
        return unicodedata.normalize("NFC", value)
    if isinstance(value, list):
        return [nfc(v) for v in value]
    if isinstance(value, dict):
        return {nfc(k): nfc(v) for k, v in value.items()}
    return value


def encode(obj: dict) -> bytes:
    # canonical=True sorts map keys by their encoded bytes (RFC 8949 4.2.1 for short text keys)
    return cbor2.dumps(nfc(obj), canonical=True)


def sign(obj: dict, key: SigningKey) -> dict:
    unsigned = {k: v for k, v in obj.items() if k != "sig"}
    message = b"nevo/1/" + obj["t"].encode() + encode(unsigned)
    return {**unsigned, "sig": key.sign(message).signature}


def verify(obj: dict) -> bool:
    from nacl.exceptions import BadSignatureError
    from nacl.signing import VerifyKey
    unsigned = {k: v for k, v in obj.items() if k != "sig"}
    message = b"nevo/1/" + obj["t"].encode() + encode(unsigned)
    try:
        VerifyKey(obj["author"] if "author" in obj else obj["key"]).verify(message, obj["sig"])
        return True
    except BadSignatureError:
        return False


def shuffled(obj: dict) -> dict:
    items = list(obj.items())
    random.shuffle(items)
    return dict(items)


def fake_id(seed: str) -> bytes:
    return object_id(seed.encode())


def main():
    author = SigningKey(bytes(range(32)))
    pub = bytes(author.verify_key)

    first = sign({
        "t": "rev", "v": 1,
        "parents": [],
        "author": pub,
        "clock": 1,
        "time": {"ms": 1_792_000_000_000, "src": "unknown"},
        "kind": "guide",
        "title": "How to build a rocket stove",
        "lang": "en",
        "topics": ["energy/cooking", "build"],
        "license": "CC-BY-SA-4.0",
        "vis": "public",
        "content": [{"name": "guide.md", "blob": fake_id("guide v1"), "size": 8_412, "mime": "text/markdown"}],
    }, author)
    first_id = object_id(encode(first))

    edit = sign({
        "t": "rev", "v": 1,
        "item": first_id,
        "parents": [first_id],
        "author": pub,
        "clock": 2,
        "time": {"ms": 1_792_000_500_000, "src": "user"},
        "kind": "guide",
        "title": "How to build a rocket stove",
        "lang": "en",
        "topics": ["energy/cooking", "build"],
        "license": "CC-BY-SA-4.0",
        "vis": "public",
        "content": [
            {"name": "guide.md", "blob": fake_id("guide v2"), "size": 9_120, "mime": "text/markdown"},
            {"name": "diagram.webp", "blob": fake_id("diagram"), "size": 48_200, "mime": "image/webp"},
        ],
        "source": {"title": "Rocket stove plans", "url": "https://example.org/rocket", "author": "A. Builder"},
        "summary": "A fuel-efficient stove from bricks or tin cans, with a materials list.",
    }, author)

    att = sign({
        "t": "att", "v": 1,
        "about": object_id(encode(edit)),
        "claim": "verified",
        "topics": ["energy/cooking"],
        "author": pub,
        "clock": 3,
        "time": {"ms": 1_792_001_000_000, "src": "unknown"},
    }, author)

    key_rec = sign({
        "t": "key", "v": 1,
        "key": pub, "kind": "person", "name": "Maria (Clinic volunteer)",
        "recovery": bytes(SigningKey(bytes(range(64, 96))).verify_key),
        "clock": 1, "time": {"ms": 1_792_000_000_000, "src": "unknown"},
    }, author)

    # Entries are always stored and sent grouped by log, so the node key lives in the log head.
    log_entry = {"seq": 1234, "prev": fake_id("entry 1233"), "obj": first_id}

    print("Sizes (bytes, deterministic CBOR):")
    for name, obj in [("first revision", first), ("edit revision (2 files, source, summary)", edit),
                      ("attestation", att), ("key record", key_rec), ("log entry", log_entry)]:
        print(f"  {name:<42} {len(encode(obj)):>4}")

    # Determinism: shuffling key order must not change bytes or ID
    for obj in (first, edit, att, key_rec):
        assert encode(shuffled(obj)) == encode(obj)
    print("Determinism: same bytes and ID regardless of key order: OK")

    # Unicode: composed and decomposed "é" must give the same ID
    a = encode({"t": "x", "title": "café"})
    b = encode({"t": "x", "title": "café"})
    assert object_id(a) == object_id(b)
    print("Unicode: NFC normalization gives one ID for both forms of 'é': OK")

    # Signatures
    assert all(verify(o) for o in (first, edit, att, key_rec))
    tampered = {**edit, "title": "How to build a rocket stove (now with petrol!)"}
    assert not verify(tampered)
    retyped = {**att, "t": "rev"}
    assert not verify(retyped)
    print("Signatures: verify, and fail after a change or a type swap: OK")

    print(f"\nItem ID as text: {id_text(first_id)} ({len(id_text(first_id))} chars)")
    print(f"Key as text:     k{b32(pub)} ({len(b32(pub)) + 1} chars)")

    # Budget for a library: per item ~2 revisions, ~1 attestation, 3 log entries
    per_item = len(encode(first)) + len(encode(edit)) + len(encode(att)) + 3 * len(encode(log_entry))
    print(f"\nMetadata per item (2 revisions, 1 attestation, 3 log entries): {per_item} bytes")
    for n in (10_000, 100_000, 1_000_000):
        print(f"For {n:>9,} items: {per_item * n / 1e6:6.0f} MB before database indexes")


if __name__ == "__main__":
    main()
