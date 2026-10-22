# NEVO Decisions Log

Major decisions and the reasoning behind them. When a decision changes, add a new entry that supersedes the old one instead of editing history.

Format: **number, decision**, then why, plus alternatives that were considered.

---

## Product

### D-001: Build NEVO as one unified system, reusing existing formats
*2026-10-21*

Kiwix, Internet-in-a-Box, RACHEL and Project N.O.M.A.D. already cover parts of this idea, but as separate tools. NEVO's gap to fill is unified search, easy community contributions, an AI that answers from the library with citations, and node-to-node sync. Existing formats such as Kiwix ZIM are reused rather than reinvented.

### D-002: Open source on GitHub
*2026-10-21*

A survival-knowledge project only earns trust if anyone can inspect it, fork it and keep it alive without its original authors. A GitHub organization (rather than a personal repository) lets the project outlive any single maintainer.

### D-003: Code license is AGPL-3.0
*2026-10-21*

People will sell pre-loaded NEVO boxes. AGPL requires anyone who modifies and distributes or serves NEVO to publish their changes, so improvements flow back to everyone.
*Considered:* Apache-2.0, which is friendlier to companies but lets forks stay closed.

### D-004: Content and models keep separate licenses
*2026-10-21*

Community contributions default to CC BY-SA 4.0, the same as Wikipedia, so content flows freely between nodes. Each knowledge pack records its own license. Bundled AI models should have permissive licenses (Apache 2.0, MIT) to avoid redistribution problems.

### D-005: Strictly offline
*2026-10-21*

NEVO never connects to the internet: it talks only to local addresses and refuses the public internet. This makes it simpler, more private and "air-gap friendly by design." Content and updates arrive by USB or Wi-Fi upload, carried in by people.
*Considered:* internet as an optional sync transport. Rejected to keep the product honest to its mission and its attack surface small.

### D-006: Whoever sets up a node fills it
*2026-10-21*

People download material on another computer and bring it in by USB, or upload it from a browser over the node's Wi-Fi. NEVO itself never downloads anything.

### D-007: A separate companion tool, NEVO Seed
*2026-10-21*

People preparing a node need to know what to download and get it onto a stick correctly. Seed runs on an online computer, downloads and prepares content, and writes a ready-to-import USB seed. It is the only part of the project that goes online.
*Considered:* documentation and links only.

### D-008: Anonymous reading
*2026-10-21*

Anyone on the Wi-Fi can browse, search and ask the AI without an account. In a crisis, a login screen is a barrier. Accounts are needed only to contribute.

### D-009: Private and node-only content
*2026-10-21*

Families can keep personal records (medical history, land records, certificates) on a node, and these never sync. The revision model already supports it. Private content is hidden, not encrypted, at first; encryption comes after v1.0.

### D-010: Bulletin board in v2
*2026-10-21*

Local, current information (needs, offers, alerts) is valuable after a collapse. Posts are items with an expiry date and sync like everything else. Deferred to v2 to keep v1 focused on the library and AI. Chat and direct messaging are out of scope.

## AI

### D-011: Don't train a model from scratch
*2026-10-21*

Pretraining is extremely expensive and unnecessary. NEVO uses existing small open models with retrieval-augmented generation (RAG). Later, it fine-tunes one into a "NEVO model" for the exact job: cite sources, give clear steps, say "I don't know."

### D-012: The AI is built in on every node
*2026-10-21*

Explaining what to do is central to NEVO, not an add-on. Every node runs the model itself. The AI grounds its answers in the node's own library, including community content, which becomes usable within minutes with no retraining.
*Supersedes:* an earlier idea of the AI as an optional sidecar.

### D-013: Minimum hardware is about 4 GB of RAM; no relay nodes
*2026-10-21*

The smallest useful model needs about 1 GB of RAM, so Pi Zero-class devices can't take part. Every node is a full node. Tiers change answer quality, not features: Standard 4 GB, Enhanced 8 GB, Power 16 GB+. Windows needs about 8 GB in practice.
*Considered:* letting weak devices relay AI questions to a nearby full node. Rejected to keep one simple kind of node with no dependencies.

### D-014: AI trust rules
*2026-10-21*

In safety-critical topics (medical, electrical, chemicals, food safety), the AI ignores content labeled Unknown, prefers Verified and Reference sources, and points out disagreements between sources.

### D-015: Semantic search is limited on very large packs
*2026-10-21*

Embeddings for all of Wikipedia would take tens of gigabytes, too much for small hardware. Huge ZIM packs use keyword search plus a reranker; semantic search covers curated packs and community content.

## Technology

### D-016: Rust core with llama.cpp compiled in
*2026-10-21*

Rust gives C-like speed and low memory use, prevents whole classes of memory bugs (vital for a box that must run for years untouched), produces a single binary per platform and has strong libraries (Axum, Tantivy, SQLite bindings). llama.cpp is the most optimized CPU inference engine and covers ARM, x86 and several GPU types.
*Considered:* C (fast but error-prone and slow to develop), Go (simple, but clumsier with llama.cpp and garbage-collected), Python (approachable but heavy on small hardware), and pure-Rust inference engines (worth watching, currently behind llama.cpp on CPU).

### D-017: Platforms
*2026-10-21*

Linux ARM and x86, and Windows, first; macOS in v0.2; Android after v1.0. Old Android phones are a promising future NEVO device.

### D-018: Server-rendered web pages
*2026-10-21*

Pages are built on the node and work without JavaScript, so very old phones and browsers can use NEVO.

## Content, sync and trust

### D-019: Content model
*2026-10-21*

Content-addressed blobs (BLAKE3, ~1 MB chunks), immutable signed revisions forming a version tree, stable items, separate signed attestations, and recomputable derived data. This gives deduplication, tamper evidence, conflict-free offline editing and easy sync.

### D-020: Logical clocks for ordering
*2026-10-21*

Offline devices often have the wrong time (a Pi has no clock battery). Ordering uses logical clocks; wall-clock time is stored as a best guess. An optional GPS module can provide true time.

### D-021: Sync is intranet-only and log-based
*2026-10-21*

Each node keeps an append-only log of what it created. Nodes compare log positions and exchange what's missing. Metadata always syncs; files sync by policy. Transports are LAN, Wi-Fi mesh and rooftop links, never the internet.

### D-022: USB bundles also carry node-to-node sync
*2026-10-21*

The `.nevo` format already exists for Seed. Reusing it to carry sync between nodes out of network range costs almost nothing.

### D-023: Local, topic-scoped trust
*2026-10-21*

Each admin chooses whom to trust and for which topics. Trust is never counted, which defeats fake accounts. Labels shown to users: Reference, Verified, Community, Unknown.

### D-024: Cautious default trust
*2026-10-21*

A new node trusts only the catalogs that signed its seed. Content from paired nodes arrives as Community until the admin promotes the source. The setup wizard offers to add a trusted curator by QR code.

### D-025: Multiple catalogs, no single authority
*2026-10-21*

The NEVO project catalog is built in, and community catalogs (NGOs, co-ops, schools) can be added, each with its own signing key. If the NEVO project disappears, community catalogs keep working.

## NEVO Seed

### D-026: Global scope for v1
*2026-10-21*

Kits are global and organized by topic. Regional kits (local plants, climate-specific farming) may come later.

### D-027: Website capture, done responsibly
*2026-10-21*

Seed can save websites into packs as WARC (the Internet Archive's format). It crawls politely and uses a pre-approved list of openly licensed sites. Capturing other sites requires the user to confirm they have the right to save them.

### D-028: Vault mode in v1
*2026-10-21*

Unpowered flash storage slowly loses data. Vault mode marks long-term drives, reminds users to re-verify about yearly, rewrites weak data, and recommends two copies on different media plus a printed guide.

### D-029: Pre-digest on the online computer
*2026-10-21*

OCR, transcripts and embeddings take hours or days on a 4 GB Pi but minutes on a desktop. Seed does this work before writing the stick, so nodes get content ready to use. Seed and the node must share the same embedding model version, which is one reason they share a codebase.

### D-030: Every tier plays video
*2026-10-21*

Playback is cheap; processing is expensive. Seed pre-digests videos into a light format, so even 4 GB nodes only stream them. Videos uploaded directly to a node are processed slowly, when power allows.

## AI model and safety (Phase 0 benchmark)

### D-031: Standard-tier model is Qwen3.5 4B, pending a Pi test
*2026-10-22*

Seven Apache-2.0 models were tested on 20 grounded-answer cases and in a 4 GB, 4-core container (see `spikes/ai-benchmark/RESULTS.md`). Qwen3.5 4B (Q4_K_M) scored 143/160, had the best behavior score, and peaked at 3.4 GB. Qwen3.5 2B (1.7 GB, 114/160) is the fallback if the 4B model is too slow or too tight on a real Pi.
*Considered:* Gemma 4 E2B (bigger, worse, slow to load), Ministral 3 3B, SmolLM3 3B and Granite 4.2 3B (lower quality, invented facts with fake citations).

### D-032: Enhanced tier and up use Gemma 4 E4B
*2026-10-22*

Best quality in the benchmark (148/160) and the only model with no dangerous mistakes. It doesn't fit in 4 GB.

### D-033: Memory-saving llama.cpp settings on every node
*2026-10-22*

One request at a time, no prompt cache in RAM, and no repacked second copy of the weights (`-np 1 --cache-ram 0 --no-repack` in llama-server terms). This cut Ministral 3B from 4.1 GB to 2.7 GB at the same speed. ARM may repack differently, so this is rechecked on a Pi.

### D-034: Code checks on safety-critical answers
*2026-10-22*

Every model under 4B made at least one dangerous mistake, and the common ones are predictable: a lost "do not", and amounts or units changed in translation. On safety-critical topics, NEVO checks the answer in code before showing it:

1. Negations in cited sources must survive in the answer.
2. Every amount and unit must match a cited source, using a fixed unit-translation table.
3. Warnings from Verified and Reference sources are shown as exact quotes.
4. Citations must point to sources that were actually given.

If a check fails, NEVO shows the source text instead of the AI answer. Seed also prepares checked summary cards for the most important procedures.
*Considered:* relying on a bigger model or a better prompt. Rejected: even the best model is not guaranteed safe, and the checks are cheap.

## ZIM and search (Phase 0 spike)

### D-035: Read ZIM files with the pure-Rust `zim` crate
*2026-10-22*

The `zim` crate (MIT/Apache-2.0) read current Kiwix files (format 6.3) correctly, opens them in under 1 ms by memory-mapping, and reads an article in about 2 ms. Its command-line dependencies should be made optional upstream, or the crate vendored. See `spikes/zim-search/RESULTS.md`.
*Considered:* bindings to Kiwix's C++ libzim (harder to cross-compile for ARM and Windows), and writing a reader from scratch (not needed).

### D-036: NEVO builds its own Tantivy index that points into the ZIM
*2026-10-22*

The ZIM's built-in Xapian index can't be read from Rust, and NEVO needs one index across ZIMs, PDFs and community content. The index stores paths, titles and redirects as aliases; text is read from the ZIM when needed. It is about a third of the ZIM's size. A 4 GB node indexes a 1 GB pack in about 35 s on one desktop core (minutes on a Pi) using about 0.3 GB, so Seed builds indexes ahead of time when it can (D-029), and nodes index only while the AI model is unloaded.

### D-037: Search ranking: stop words, exact title match, then a reranker
*2026-10-22*

Plain keyword ranking put *Panic attack* above *Myocardial infarction* for "heart attack". Dropping stop words and putting exact title or redirect matches first fixed most test queries. A small reranker comes next, and questions in a language other than the pack's are translated by the built-in AI before searching. The AI receives short passages, not whole articles, so the index also needs passage-level entries.

## Content format

### D-038: Content format version 1 is locked
*2026-10-22*

`docs/spec/content-model.md` is format version 1: BLAKE3 IDs in IPFS-compatible multihash form, Ed25519 signatures, deterministic CBOR, and the revision, attestation, key, derived-data, log and `.nevo` bundle formats. Choices made in review: one key per person in v1 (signed device keys in v2), content always in blobs, private content hidden but not encrypted, and topics from a fixed starting list that catalogs extend with sub-topics. Later versions may add fields but must keep version 1 objects readable, with their IDs and signatures valid.

### D-039: Format v1 clarifications after the first code review
*2026-10-22*

Building `nevo-format` and `nevo-store` and having them independently reviewed showed gaps in the locked spec (D-038). The spec now states:

- **Forward compatibility:** later spec versions may add values for revision kinds, claims, time sources, key kinds and top-level topics. Version 1 nodes store and pass them on, but don't trust them. Only a new visibility value needs a new format version.
- **Limits:** counters and sizes are at most 2^63 − 1.
- **Attestations:** `verified` needs at least one topic, like `trust`.
- **Revocation:** a recovery key named in any key record for the key may revoke it, so a key thief can't block the owner by publishing a new record.
- **Logs:** unsigned log entries from another node are accepted only up to a head it signed. All signed heads are kept, and contradicting heads are kept as proof of a fork.

No version 1 data existed yet, so nothing had to be migrated. This supersedes the affected details of D-038.

## Accounts and security

### D-050 (Proposed): Passphrases unlock per-person keys with Argon2id
*2026-10-22*

Each account has its own Ed25519 key (D-038). The secret key is stored encrypted with XChaCha20-Poly1305 under a key derived from the passphrase with Argon2id (19 MiB, 2 passes, 1 lane: the OWASP minimum and Argon2's default). A copy of the data folder gives an attacker no signing keys, only a slow offline guessing job per account. The passphrase is checked by decrypting the key: there is no separate password hash, so sign-in costs one Argon2 run (about 21 ms on a desktop CPU, an estimated 150-250 ms on a Raspberry Pi 4). At most two Argon2 runs happen at once, so sign-ins can't exhaust a Pi's memory. The cost is stored per account and can be raised later. Accounts live in `accounts.sqlite`, apart from the library: they are local and never sync. A forgotten passphrase can't be reset; the person gets a new account and key.

Alternatives: a password hash plus a separately stored key (two secrets to keep in step, and a stolen folder still holds the key); bcrypt or scrypt (Argon2id is the current recommendation).

### D-051 (Proposed): Cookie sessions and forged-form protection on a plain-HTTP network
*2026-10-22*

Sign-in sets a random 256-bit cookie, `HttpOnly` and `SameSite=Strict`, valid 30 days. It can't be `Secure`: a NEVO is reached over plain HTTP on a local network without a certificate. Anyone who can watch the Wi-Fi traffic can therefore copy a session (or a passphrase as it is typed); WPA2/3 Wi-Fi encryption is the protection for now, and HTTPS with a node certificate is a later step. The database stores only a hash of the cookie, plus the person's key encrypted under another value derived from the cookie, so sessions survive a restart but the database alone can't sign. Every POST must carry the session's form token in a hidden field and come from the node itself by its `Origin` header (or `Referer` for old browsers). For that, pages use `Referrer-Policy: same-origin` instead of `no-referrer`, which makes browsers send `Origin: null` on every form post. Forms used before sign-in use a random cookie compared with the hidden field.

### D-052 (Proposed): The setup wizard needs a one-time code from the console
*2026-10-22*

Until a node has an admin, every page leads to `/setup`, where the first person names the node and creates the admin account. Anyone on the network could reach it first and take the node over, so `nevo` prints a one-time code (12 characters, about 59 bits, without look-alike characters) in its console at startup, and `/setup` requires it. Only someone who can see the console, normally the person who started the node, can finish setup. The node's own key record, with its name, is published at the same time.

### D-053 (Proposed): Moving a key to another device by QR code
*2026-10-22*

Following D-038 (one key per person, moved by QR code), the account page shows the secret key as an inline SVG QR code and as text, only after the passphrase is typed again: a session alone (a borrowed phone, a copied cookie) must not be enough to take someone's key. The text form is `NEVO-SECRET-1:` followed by the 32 bytes in uppercase RFC 4648 base32 without padding, which QR codes store compactly in alphanumeric mode. Importing on the other device comes with the app or a second node's sign-in.
