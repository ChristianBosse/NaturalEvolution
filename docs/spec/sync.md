# NEVO sync protocol

**Status: draft, protocol version 1, proposed 2026-10-22 (D-041 to D-043).** Implemented in `crates/nevo-sync`, except pairing requests and discovery (sections 2 and 3), which are specified here for the admin pages to build on. Open questions are listed in section 13.

This spec says how NEVO nodes on the same local network find each other, decide whom to talk to, and exchange logs, objects and blobs. It builds on the content model ([content-model.md](content-model.md)), especially section 4 (logs, signed heads, forks). USB bundles (D-022) carry the same data and are specified there.

## Goals

1. **Trust no sender.** Everything received is checked: objects by their IDs and signatures, log entries by the hash chain up to a head their own node signed, blobs by their hash. A relay can withhold data but can't change it.
2. **Local network only.** Sync uses LAN, Wi-Fi mesh and rooftop links, never the internet (D-005, D-021).
3. **Safe to cut off.** A link can drop or the power can fail at any moment. The next round continues from what was stored, and nothing half-checked is ever kept.
4. **Knowledge spreads by contact.** A node passes on every log it holds, so content reaches nodes its author never met.
5. **Small enough to reimplement.** Plain HTTP, the same CBOR as objects, Ed25519 signatures, five requests.

## 1. Overview

Sync is **pull only**: a node asks a peer what it has and fetches what it lacks. Both directions of an exchange are two pulls, one by each side. Pulling means a node only ever stores what it chose to fetch and checked.

One pull round from a peer:

1. **Heads:** fetch the peer's highest signed head for each log it holds. Compare with "how far do I have that log" (the highest `seq` stored).
2. **Entries:** for each log the peer has further, fetch the missing entries. Entries are stored only in batches that end at a signed head.
3. **Objects:** fetch every object listed in a stored log that this node doesn't have yet.
4. **Blobs:** fetch the files that the local blob policy wants: by topic, size, trust and pinned items.

Metadata (heads, entries, objects) always moves. Blobs move by policy (D-021).

## 2. Pairing

Only **paired** nodes may pull from each other. A pairing is approved once by an admin on each side and stays until removed.

Each node has a node key (Ed25519, content-model section 1). Pairing means each node adds the other's public key to its **paired peer list**. Nothing secret is exchanged: requests are signed with the node key, and the other side checks the signature against its list (section 5).

**By QR code** (preferred, nothing to type). The admin page of node A shows a QR code holding:

```text
nevo-pair:1?key=k<A's key>&addr=http://192.168.1.20:8080/sync&name=School%20library
```

The admin of B scans it, checks the name, and approves: B adds A. B then sends A a pairing request (below), and A's admin approves it, comparing B's key fingerprint on screen. The QR code carries A's full key, so no one on the network can slip in another key.

**By short code** (when there is no camera). A's admin page shows its address, its key fingerprint (the first 8 characters of the key text after `k`) and a one-time code of 10 base32 characters, valid for 10 minutes. The admin of B types the address and code into B. B fetches `/sync/hello`, shows A's fingerprint to its admin to compare with A's screen, and sends the pairing request. A accepts the request only with a valid proof of the code, then asks its admin to approve B, showing B's fingerprint.

**Pairing request** (`POST /sync/v1/pair`, signed like every request but accepted from unpaired keys): B's key, name and address, plus `proof` = keyed BLAKE3 of the code over A's key and B's key when a short code is used. The code itself is never sent, and the proof binds it to the two keys each side sees. *Not implemented yet; see open question 3.*

**The list** is a text file, one peer per line: key, address (or `-`) and name. Admins can read and fix it by hand. A removed peer is refused from its next request on.

```text
# NEVO paired peers: key, address (or -), name
kq2x…7ma http://192.168.1.20:8080/sync School library
kd9v…p3e - Clinic
```

A new node pairs with no one. Content from paired nodes arrives as Community until the admin promotes the source (D-024); pairing decides who may sync, not whom to trust.

## 3. Discovery

Discovery finds *candidates*. It never grants access: a discovered node is ignored until paired. It is **optional** and can be off; a typed address always works.

**Recommended: mDNS/DNS-SD**, the standard behind Bonjour and Avahi. A node advertises the service `_nevo._tcp.local` on its web port with TXT records `v=1` (protocol versions, comma-separated), `key=k…` (node key) and `path=/sync`. It is a standard that phones, laptops and Linux tools already understand and debug, and every platform has a library for it.

How it is used:

- A paired peer's address is updated when its key shows up at a new address (DHCP leases change). The new address is confirmed through `/sync/hello` before use.
- The pairing page lists discovered nodes not yet paired, to save typing.

mDNS uses link-local multicast. It works on one LAN and across bridged meshes (batman-adv, for example) but not across routed links such as most rooftop point-to-point setups. There, admins type the address once.

*Considered:* a plain UDP broadcast on a fixed port. It is simpler, but it is not a standard, needs its own tools, and has the same one-link reach.

## 4. Transport and messages

- **HTTP/1.1 over TCP**, on the node's normal web port, under the path `/sync`. HTTP works through every Wi-Fi and mesh setup and is easy to debug.
- **No TLS.** A LAN has no certificate authority, and self-signed certificates teach people to click through warnings. Signatures (section 5) give authenticity; see open question 4 for privacy on the wire.
- **Bodies are deterministic CBOR maps**, the encoding objects already use. Objects, entries and blob parts are carried as CBOR byte strings, exactly as stored: no re-encoding and no base64 (which JSON would need, at a third more size). One decoder serves objects and messages. Unknown map keys are ignored, so later versions can add fields.

| Request | Body | Answer |
|---|---|---|
| `GET /sync/hello` | none | `versions` (array of integers), `node` (key), `challenge` (16 random bytes) |
| `POST /sync/v1/heads` | `after` (key, optional), `limit` | `heads` (array of encoded heads), `forks` (array of encoded heads), `next` (key, when there are more) |
| `POST /sync/v1/entries` | `node`, `after`, `upto` | `entries` (array of encoded entries), `head` (encoded head the entries end at, optional) |
| `POST /sync/v1/objects` | `ids` (array of IDs) | `objects` (array of encoded objects), `done` (how many IDs were looked at) |
| `POST /sync/v1/blob` | `id`, `from`, `len` | `size` (whole blob), `data` (bytes from `from`) |

Keys are 32-byte strings, IDs 34-byte multihashes, as in the content model. Answers have the content type `application/cbor`.

## 5. Authentication

Every `/v1/` request is signed by the requesting node; every answer is signed by the serving node.

**Requests.** The client gets a challenge from `/sync/hello` and keeps a counter that starts at 1 and grows with every request under that challenge. It signs this map, exactly like an object (`"nevo/1/"` + type, content-model section 1):

| Field | Value |
|---|---|
| `t`, `v` | `"sync-req"`, `1` |
| `node` | the client's node key |
| `ch` | the challenge |
| `n` | the counter |
| `op` | `heads`, `entries`, `objects` or `blob` |
| `body` | BLAKE3 of the request body |

The request carries four headers: `nevo-node` (key text), `nevo-challenge` (hex), `nevo-count` (decimal) and `nevo-sig` (hex). The server accepts it only if the key is paired, the signature is valid, the challenge is one it issued and still remembers, and the counter is higher than any used before under that challenge. The first key to use a challenge owns it.

No clocks are involved: a Pi often has the wrong time (D-020). A server forgets challenges after 15 minutes unused, and on restart; the client then gets 401 and asks `/hello` again.

**Answers.** The server signs `t: "sync-res"`, `v: 1`, `node` (its key), `req` (the request's signature) and `body` (BLAKE3 of the answer body), and sends the signature in `nevo-sig`. The client checks it against the key it paired with. This binds each answer to its request and to the paired node, so another machine on the network can't answer in its place. `/hello` itself is not signed; the client compares its `node` with the paired key, and every later answer proves it.

The domain types `sync-req` and `sync-res` are not object types, so a sync signature can never pass as content.

**Status codes:**

| Code | Meaning |
|---|---|
| 400 | Malformed request |
| 401 | Bad signature, or unknown, expired or reused challenge: ask `/hello` again |
| 403 | Not paired, or the request came from outside the local network |
| 404 | Not available: missing, or not allowed to leave this node (the same answer for both) |
| 413 | Request too large |
| 429 | Too many requests from this peer: slow down |
| 503 | Busy: try again soon |

## 6. A pull round

**Heads.** The client pages through `heads` (500 logs per page). For each log, the server sends its highest signed head *for which it has all entries*, so it can serve everything the head covers. Before answering, a server signs a fresh head of its own log if it has published since its last head.

The client stores each head (which checks its signature and finds forks, section 8) and compares its `seq` with its own position in that log. Its own log is skipped: the node writes that itself.

**Entries.** For each log that is behind, the client asks for entries after its position, up to the head's `seq`. The server sends at most 4096 entries, and ends them at the highest signed head in that range when there is one, sending that head along. The client:

1. checks that each entry decodes and continues the sequence;
2. keeps entries in memory until they reach a signed head (the one sent along, or the target head);
3. then stores the batch. The store checks the hash chain from the head back to what it had, and takes all entries or none.

A peer that never reaches a head can make a client hold at most 100,000 entries (about 9 MB) before the client refuses that log for the round.

**Objects.** The client walks the objects listed in its logs that it doesn't have, in ID order, 256 IDs per request. For each object received it checks:

- its hash is one of the IDs asked for, so it is exactly the object a signed log listed;
- it is a valid object with a valid signature;
- it is not a node-only or private revision.

The server stops adding objects at 8 MiB and says, in `done`, how many IDs it looked at; the client asks again for the rest. IDs the peer doesn't have stay missing for another peer to provide.

**Blobs.** For each blob that a stored public revision uses and this node lacks, the client asks its **blob policy**, giving the size and the revisions that use it. The default policy takes a size limit, topics, trusted authors (filled in by the trust layer) and pinned items or blobs.

Blobs move in parts of up to 4 pieces of 1 MiB, starting at piece boundaries. The size the peer reports must match a size the revisions claim, so a peer can't make a client download more than promised. Parts go to a partial file in `sync-partial/`, which survives restarts. When the file is complete, it is hashed and kept only if it matches the blob ID; otherwise it is deleted and the problem reported.

**Errors.** Connection and storage errors end the round. Anything wrong with what the peer sent (a forged head, entries that don't chain, a corrupt blob) is refused item by item and listed in the round's report; the round goes on.

## 7. What a node refuses

**As a server**, a node never sends:

- **node-only and private content** (D-009): the store only offers objects listed in a log or that are log entries and heads (`shareable_object_bytes`), and its own log never lists unshareable objects. The server also drops any revision that isn't public, in case another node's log lists one of ours.
- **blobs**, unless a published public revision uses them *and* no node-only or private revision on this node does. The second rule stops a probe: a peer that knows a file's hash could publish a revision naming it and learn from the answer whether someone keeps that file privately here.
- anything to an **unpaired** key, an **unsigned** or **replayed** request, or a request from **outside the local network** (when the node serves with connection info).

**As a client**, a node refuses: peers outside the local network (every address a name resolves to must be local: loopback, RFC 1918, link-local, RFC 6598 shared space, IPv6 unique-local); answers not signed by the paired key; forged or contradicting heads; entries that don't chain to a signed head; objects not asked for or not matching their ID; node-only and private revisions; blobs whose size or hash is wrong; copies of its own log.

## 8. Forks

A node forks when it signs two different logs (for example after restoring an old backup and publishing again). Two signed heads, or a head and an accepted entry, that disagree at the same position are the proof (content-model section 4).

- **Detection:** storing a head checks it against the heads and entries already stored. A client stores every head it receives, even for logs it is ahead in, so contradictions are found.
- **Report:** each round lists the nodes found forked. The proof is kept, and the admin page shows it. What it means for that node's content is for the trust layer to decide.
- **Passing on:** the `heads` answer includes, in `forks`, the signed heads that prove each fork. A client stores them *after* the honest log's entries, so a contradicting head is checked against those entries and can't block the log from being accepted first. A fork found against an entry has only one head in `forks`; the receiver finds the contradiction against its own copy of the log.

## 9. Knowledge spreads by contact

Every node serves every log it holds, up to the heads it holds, together with the objects those logs list and the blobs it may share. So when node C pulls from B, it receives A's log too, although it never met A. Everything is checked against A's signature, never B's: a relay can delay or withhold A's content but can't change it.

Trust in relayed content follows its author and the node that published it, not the relay (D-023).

## 10. Resuming

A round can stop at any moment, and running it again is harmless:

- log positions move only when a batch ending at a signed head is stored, in one transaction;
- "listed but missing" objects are a store query, so they are found again;
- blob downloads continue from the last whole piece of the partial file. A piece cut short by a power loss is fetched again;
- a finished download is kept only after its hash matches.

Two rounds into the same node at the same time are safe for the data, but they waste work; a node should run one at a time.

## 11. Limits

| Limit | Default | Why |
|---|---|---|
| Request body | 64 KiB | Requests are short lists |
| Answer body | 16 MiB | One blob part plus framing |
| Heads per answer | 500 | About 80 KB |
| Entries per answer | 4096 | About 400 KB |
| IDs per `objects` request | 256 | |
| Objects per answer | stops at 8 MiB | Objects are at most 1 MiB each |
| Blob part | 1 to 4 pieces of 1 MiB | Aligned to the pieces of content-model section 2 |
| Concurrent sync requests served | 4 | Keeps a Pi responsive for readers |
| Requests per peer | 20 per second, bursts of 100 | |
| Per round (client) | 1,000,000 entries, 100,000 objects, 4 GiB of blobs | Each settable |
| Timeouts (client) | 10 s to connect, 120 s per request | A 4 MiB part over a 1 Mbit/s rooftop link takes 34 s |

## 12. Versioning

- The protocol version is in the path (`/sync/v1/…`) and in every signed map (`v`). `/sync/hello` lists the versions a node speaks; the client uses the highest one both know.
- **Compatible changes** (new optional fields, new requests) keep version 1. Old nodes ignore unknown fields and get 404 for unknown requests.
- **Breaking changes** get a new version under a new path (`/sync/v2/…`). Nodes serve both versions during a transition, since rural networks upgrade slowly.
- The protocol version is separate from the content format version (D-038). A v1 protocol can carry objects of later format versions, which nodes store and pass on.

## 13. Open questions

1. **Push.** Pull only means new content waits for the other side's next round. Should a node be able to say "please pull me now", and how often should rounds run (on discovery, on publish, every few minutes)?
2. **Piece-by-piece verification.** Content-model section 2 checks each 1 MiB piece against the blob's BLAKE3 tree using Bao outboards. The store has no outboards yet, so v1 checks the whole blob at the end; a single bad piece costs the whole download. Adding outboards changes nothing on the wire: parts are already aligned to pieces, and the outboard can be fetched like a blob.
3. **Short-code strength.** A 10-character code (50 bits) gives an attacker on the LAN only a few minutes to guess it offline from the proof. Is that enough, or should pairing use a PAKE (such as SPAKE2)? The QR code doesn't have this problem. Browsers allow camera access only over HTTPS or on localhost, so scanning the QR code from the admin page may need the phone's own camera app.
4. **Privacy on the wire.** Public content is public, and private content never travels, but anyone on the LAN can see which public items a node fetches. Should v2 encrypt the link (Noise with the node keys)?
5. **Heads for long logs.** A node signs a head when asked for its heads, so a log built offline over months may have one head covering thousands of entries, and relays can only cut it at the heads they hold. Should nodes also sign a head every 1000 entries?
6. **Relay trust labels.** Content from paired nodes starts as Community (D-024). Should content their logs relay from unknown nodes start as Unknown?
7. **The peer list** is a text file next to the store. Should it move into the store's database once the admin pages exist?
8. **Derived data blobs** (D-029) are not served yet: only blobs that revisions use are.

## 14. Choices made

1. **Pull only, over plain HTTP, with CBOR bodies** (sections 1 and 4).
2. **Signatures instead of TLS**, with server challenges and counters instead of timestamps (section 5).
3. **mDNS/DNS-SD for optional discovery** (section 3).
4. **Whole-blob verification in v1**, piece by piece once the store keeps outboards (section 6, open question 2).
