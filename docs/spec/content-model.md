# NEVO content model format

**Status: format version 1, locked 2026-10-22 (D-038).** Later changes must keep version 1 objects readable and their IDs and signatures valid. New fields may be added; existing fields can't change meaning.

This spec turns decisions D-019 to D-025 into exact formats. It covers how content, versions, signatures and trust statements are stored and identified. How nodes find each other and exchange data (the sync protocol) is a separate spec; this one only defines the log entries sync relies on.

## Goals

1. **Survive decades.** Someone with this document and no NEVO code can read a library.
2. **Verify without trusting the sender.** Every object can be checked offline: its ID is its hash, and signed objects carry the signer's key.
3. **Work fully offline.** No central registry, no clocks that must be right, no online key servers.
4. **Small enough for a Pi.** A knowledge pack such as Wikipedia is one item, not one per article, so a library has thousands of items, not millions. Metadata for 100,000 items takes about 160 MB (see section 7).

## 1. Building blocks

| Choice | Format | Why |
|---|---|---|
| Hash | BLAKE3, 32 bytes | Fast on a Pi, and its tree structure lets large files be verified piece by piece |
| Signature | Ed25519: 32-byte public key, 64-byte signature | Small, fast, deterministic, available in every language |
| Encoding | Deterministic CBOR (RFC 8949 §4.2.1) | Compact binary JSON with one exact byte form per value, readable by libraries in every language |
| ID format | Multihash: `0x1e 0x20` + 32-byte BLAKE3 digest (34 bytes) | The first bytes name the hash function, so a future hash can be added without ambiguity. IPFS-compatible. |
| ID as text | `b` + lowercase base32 of the 34 bytes (multibase) | Case-insensitive, safe in file names and URLs, can be read aloud |
| Key as text | `k` + lowercase base32 of the 32-byte public key | Short enough for a QR code |

### Encoding rules

- Only these CBOR types are used: unsigned and negative integers, byte strings, text strings (UTF-8, NFC-normalized), arrays, maps, `true`, `false`, `null`.
- **No floats.** Numbers that need decimals are stored as integers in a stated unit, for example milliseconds or micrometres.
- Counters and sizes (`clock`, `seq`, `from`, file `size`) are at most 2^63 − 1, so they fit a signed 64-bit database integer.
- Map keys are short text strings, sorted as the RFC's deterministic rules require. Unknown keys are kept and ignored, so newer nodes can add fields without breaking older ones.

### Forward compatibility

Later spec versions may add new values without changing the format version, and version 1 nodes must still store and pass on objects that use them:

- new revision `kind` and attestation `claim` values (shown as unknown)
- new `time.src` values (treated as `unknown`)
- new key `kind` values (never trusted by default)
- new top-level topics (stored, but no trust applies to them)

A new value of `vis` does need a new format version, because a node that misreads visibility could leak private content.
- No indefinite-length items and no CBOR tags.

### Object IDs and signatures

Every object is a CBOR map with `t` (type) and `v` (format version, `1`).

- **Object ID** = BLAKE3 of the object's complete encoded bytes, signature included. Ed25519 signatures are deterministic, so an object has exactly one ID.
- **Signing:** remove the `sig` field, encode the rest, prefix the domain string `"nevo/1/"` plus the type, and sign that. The prefix stops a signature made for one object type being reused for another.

## 2. Objects

### Blob

Raw bytes: a PDF, a video, a ZIM file, a Markdown guide.

- **Blob ID** = BLAKE3 of the whole file, the same value the standard `b3sum` tool prints.
- **Transfers move 1 MiB pieces.** Each piece is checked against the blob ID using a small "outboard" file of BLAKE3 tree hashes (the Bao format), about 64 bytes per MiB. Interrupted copies resume, and a corrupt piece is detected and re-fetched alone.
- Blobs carry no metadata. Name, type and license are in the revision that uses them.

### Revision (`t: "rev"`)

An immutable, signed version of an item.

| Field | Type | Meaning |
|---|---|---|
| `t`, `v` | `"rev"`, `1` | |
| `item` | ID | The item this belongs to. **Absent in the first revision**: the first revision's own ID becomes the item's ID. |
| `parents` | array of IDs | Previous revisions. Empty for the first, two or more for a merge. |
| `author` | public key | Signer |
| `clock` | integer | Logical clock: one more than the highest clock among its parents and the author's previous objects |
| `time` | map | Best-guess wall time: `ms` (Unix milliseconds) and `src` (`gps`, `network`, `user` or `unknown`) |
| `kind` | text | `guide`, `article`, `document`, `video`, `audio`, `image`, `map`, `pack`, `catalog`, `web-archive`, `dataset` |
| `title` | text | |
| `lang` | text | BCP 47 language tag, e.g. `en`, `fr`, `es-MX` |
| `topics` | array of text | Topic paths, e.g. `medical/first-aid`. These scope trust (D-023). |
| `license` | text | SPDX identifier, e.g. `CC-BY-SA-4.0` |
| `vis` | text | `public`, `node` (stays on this node) or `private` (never leaves this node, hidden from others) |
| `content` | array of maps | The files: `name`, `blob`, `size`, `mime` |
| `source` | map, optional | Where it came from: `title`, `url`, `author`, `retrieved`. Needed for license attribution. |
| `summary` | text, optional | One or two sentences |
| `sig` | 64 bytes | Ed25519 signature |

Rules:

- **Content always lives in blobs**, even a short guide (as a Markdown blob). Revisions stay small and identical files are stored once.
- **Edits never overwrite.** Two people editing offline create two revisions with the same parent. A later revision with both as `parents` merges them. Until then, a node shows the branch with the highest trust, then the highest `clock` (VISION: "Edits never overwrite").
- **Items need no registry.** An item's ID is its first revision's ID, so it can't collide or be claimed by someone else.

### Attestation (`t: "att"`)

A signed statement about a revision, an item or a key.

| Field | Type | Meaning |
|---|---|---|
| `t`, `v` | `"att"`, `1` | |
| `about` | ID or key | What the statement is about |
| `claim` | text | See below |
| `topics` | array of text | Topics the claim applies to. Required for `verified` and `trust`, optional otherwise. |
| `related` | ID, optional | For `translation-of`, `duplicate-of` and `replaces` |
| `note` | text, optional | Human explanation |
| `author`, `clock`, `time`, `sig` | | As in a revision |

Claims:

| Claim | Meaning |
|---|---|
| `verified` | I checked this revision and it is correct for these topics |
| `dangerous` | Following this could hurt someone. The note says why. |
| `outdated` | Superseded or no longer correct |
| `hide` | Don't show this by default (spam, off-topic) |
| `translation-of` | This item translates `related` |
| `duplicate-of` | Same content as `related` |
| `replaces` | This item takes over from `related` |
| `trust` | I trust this key for these topics (catalogs and curators publish these; a node only acts on them if it trusts the author) |
| `withdraw` | I take back my earlier attestation `about` |

An attestation counts only if the node trusts its author for the topic (D-023). Signatures are never counted.

### Key record (`t: "key"`)

Who a key belongs to. Self-signed by that key.

| Field | Meaning |
|---|---|
| `key` | The public key |
| `kind` | `person`, `node` or `catalog` |
| `name` | Display name |
| `recovery` | Optional second public key, kept offline (printed or on a backup stick). It can sign a revocation if the main key is lost or stolen. |
| `clock`, `time`, `sig` | |

A **revocation** (`t: "revoke"`) names a key and the clock from which its signatures are no longer valid. It is signed by the key itself or by its recovery key. Objects signed before that clock stay valid. Fields: `key` (the revoked key), `from` (the first invalid clock value), `by` (the signer: the key or its recovery key), `clock`, `time`, `sig`. A recovery key named in **any** key record ever published for the key may revoke it. Using only the latest record would let a thief publish a new record with their own recovery key and block the owner's revocation.

### Derived data (`t: "derived"`)

Results computed from a blob: extracted text, transcripts, embeddings, thumbnails, search indexes.

| Field | Meaning |
|---|---|
| `from` | Blob ID of the input |
| `producer` | Tool name and version, e.g. `nevo-ocr/1.2` or an embedding model's name |
| `out` | Blob ID of the result |
| `author`, `sig` | The node or Seed that computed it |

Derived data is not part of an item's history. Any node can recompute it and get the same `from` and `producer`. Nodes may accept it from others to save work (D-029), but the AI never treats derived text as more trusted than its source.

## 3. Packs and catalogs

These reuse revisions, so they get versions, signatures and trust for free.

- **A pack** is a revision with `kind: "pack"`. Its `content` lists the files (for example a ZIM blob and its search index), plus a `pack.cbor` blob describing them.
- **A catalog** is a revision with `kind: "catalog"`, signed by a catalog key. Its content is a list of packs with their IDs, sizes, licenses and topics. A Seed stick's `manifest` (VISION: "Seed stick layout") is a catalog revision.

## 4. Logs

Each node keeps an append-only log of the objects it publishes: its own users' revisions, attestations and key records.

A log entry is an unsigned object (`t: "log"`). Entries are always stored and sent grouped by log, so the node's key is not repeated in each one:

| Field | Meaning |
|---|---|
| `seq` | 1, 2, 3, … with no gaps |
| `prev` | ID of the previous log entry, so the log is a hash chain |
| `obj` | ID of the object |

Every so often the node signs its latest entry as a **log head** (`t: "head"`: `node`, `seq`, `entry`, `sig`). Because every entry links to the one before, a signed head vouches for the whole log up to that point.

Log entries are not signed, so a node accepts another node's entries only up to a head that node signed: the entries must chain back from that head's entry to what the node already has. Nodes keep every signed head they see. Two heads (or a head and an accepted entry) that disagree about the same position are proof that the node signed two different logs. That proof is kept and shown, and the trust layer decides what to do with that node.

Nodes keep copies of other nodes' logs, which is how knowledge spreads by contact (D-021). Sync compares "highest `seq` I have for each node" and sends the missing entries and objects. Metadata always moves; blobs move by policy. This spec only fixes the entry format; the exchange itself belongs to the sync spec.

## 5. Files on disk and bundles

**On a node** (not part of the format; may change):

- `objects.sqlite`: every object, plus indexes by item, author and topic
- `blobs/ab/cd/<blob-id>`, with `<blob-id>.obao` outboards for large blobs

**`.nevo` bundle** (part of the format, because bundles travel between nodes):

- A plain uncompressed `tar` file. Tar has been readable for 40+ years, and most content (video, ZIM) is already compressed.
- `bundle.cbor` first: a signed object (`t: "bundle"`) listing the log ranges and blob IDs inside
- `objects/<id>` and `blobs/<id>` (plus `.obao` outboards)
- Any tar tool can unpack it. Any NEVO version can check every object and blob against its ID.

**Bundle object** (`t: "bundle"`), added after the first implementation:

| Field | Type | Meaning |
|---|---|---|
| `t`, `v` | `"bundle"`, `1` | |
| `by` | public key | The node or Seed that wrote the bundle, and signer |
| `clock`, `time` | | As in a revision |
| `logs` | array of maps | One per node: `node` (key), `from` and `to` (the first and last `seq` included, `1 <= from <= to`) and `head` (ID of that node's signed log head at `to`). At most one range per node. |
| `blobs` | array of maps | `blob` (ID) and `size` (bytes), no repeats |
| `note` | text, optional | What the bundle is for, e.g. "Medical kit, October 2026" |
| `sig` | 64 bytes | Ed25519 signature |

A range ends at a signed head because receivers accept entries only up to one (section 4). The signature says who wrote the bundle; it doesn't make the content trusted, and every head, entry, object and blob is still checked on its own. Bundle objects are not stored in a library.

Members use exactly these names, with IDs in their text form: `bundle.cbor`, `objects/<id>`, `blobs/<id>` and `blobs/<id>.obao`. They come in this order, so a reader can import in one pass and a cut-off copy loses blobs before metadata:

1. `bundle.cbor`
2. For each log in `logs`: its head, its entries from `from` to `to`, then the objects those entries list that aren't already in the bundle. Objects not listed by an entry don't belong in a bundle.
3. The blobs.

A reader ignores members with any other name. Members are plain files in ustar format.

**Version 1 simplification:** outboards are optional. Version 1 writers leave them out, and readers verify each blob whole against its ID and ignore any `.obao` member.

**Export:** a dump command writes items as plain folders (`Title/` holding the files and a `meta.json`), so a library stays usable without NEVO.
Each folder holds the item's current version. `meta.json` gives its title, topics, license, source and every version. Names are made safe on Windows and Linux, and repeated titles get a number (`Title (2)`). Node-only and private items are left out unless asked for.

## 6. Topics

Topics scope trust (D-023): "I trust this curator for `medical`" must mean the same thing on every node, so v1 starts from a fixed list.

- A topic is a path of lowercase words joined by `/`, using only `a`–`z`, `0`–`9` and `-`. Example: `medical/first-aid`.
- Trust in a topic covers everything under it: trusting a key for `medical` includes `medical/first-aid`.
- Catalogs may add sub-topics under the list below, but not new top-level topics. New top-level topics come only with a new version of this spec.
- Nodes keep and show unknown sub-topics, and treat them as their parent for trust.

| Topic | Starting sub-topics |
|---|---|
| `medical` | `first-aid`, `maternal`, `child`, `medicines`, `infections`, `mental-health`, `dental` |
| `water` | `treatment`, `sanitation`, `wells` |
| `food` | `cooking`, `preservation`, `nutrition` |
| `farming` | `crops`, `seeds`, `soil`, `animals`, `fishing` |
| `energy` | `solar`, `electrical`, `fuel`, `cooking` |
| `shelter` | `building`, `heating`, `cooling` |
| `repair` | `electronics`, `vehicles`, `tools`, `textiles` |
| `safety` | `fire`, `chemicals`, `disasters` |
| `communication` | `radio`, `networks` |
| `education` | `literacy`, `math`, `science`, `languages` |
| `maps` | |
| `culture` | `history`, `books` |
| `general` | |

## 7. Measured sizes

Checked with [spikes/content-model/encode_test.py](../../spikes/content-model/encode_test.py), which also confirms that key order and Unicode form don't change an ID, and that a signature fails after any change, including a swapped object type.

| Object | Size |
|---|---|
| First revision of a guide | 385 bytes |
| Edit with two files, a source and a summary | 706 bytes |
| Attestation | 238 bytes |
| Key record | 241 bytes |
| Log entry | 89 bytes |

The Rust implementation (`crates/nevo-format`) produces byte-identical objects and the same IDs as this independent Python encoder.

With about 1.6 KB per item (two revisions, one attestation, three log entries), 10,000 items need 16 MB and 100,000 items need 160 MB, before database indexes.

An ID as text is 56 characters (`bdyqnhrr5rhm…`), and a public key is 53.

## 8. Choices made in review

Settled on 2026-10-22, all as recommended in draft 0.1:

1. **IDs are IPFS-compatible**: multihash plus multibase base32 (section 1).
2. **One key per person in v1.** A person moves their key to other devices by QR code or backup file. Signed device sub-keys come in v2.
3. **Content always lives in blobs**, even short guides (section 2).
4. **Private content is hidden, not encrypted**, in v1.
5. **Topics come from a fixed starting list** that catalogs extend with sub-topics (section 6).
