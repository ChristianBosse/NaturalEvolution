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
