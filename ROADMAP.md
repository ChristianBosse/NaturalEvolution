# NEVO Roadmap

## The MVP goal

> On a 4 GB Raspberry Pi, a non-technical person can flash a seed, boot, import a Medical kit, ask "how do I treat a burn?" from their phone, and get a clear answer with sources in about a minute.

If v0.1 does this well, NEVO's core idea is proven. Everything else builds on it.

## Why this order

1. **Prove the hardest promise first:** a useful AI on 4 GB of RAM. If it's too weak, we find out in the first weeks, not after months of work.
2. **The content model comes early, sync comes late.** Signed revisions and fingerprinted files go in from day one, so adding sync later doesn't mean rewriting everything.
3. **Every phase is usable on its own.** A v0.1 box is already valuable in a school or a cabin, without sync.

## Phase 0: Foundations

- [x] GitHub repository
- [ ] GitHub organization (optional; the repository can be transferred to it later)
- [x] AGPL-3.0 license, README, VISION, ROADMAP, DECISIONS, CONTRIBUTING, CODE_OF_CONDUCT
- [ ] **AI benchmark spike (top priority):** run llama.cpp on a 4 GB Pi 4/5 with 3–4 candidate small models. Measure speed, RAM use and answer quality on about 20 real survival and medical questions. This picks the Standard-tier model.
  - [x] Desktop run: 7 models, quality on GPU, memory and speed in a 4 GB, 4-core container. Results in [spikes/ai-benchmark/RESULTS.md](spikes/ai-benchmark/RESULTS.md); Qwen3.5 4B picked for Standard (D-031).
  - [ ] Confirm speed and memory on a real 4 GB Pi
- [x] **ZIM spike:** read and search Kiwix ZIM files from Rust. Results in [spikes/zim-search/RESULTS.md](spikes/zim-search/RESULTS.md) (D-035 to D-037).
- [x] **Lock the content model format:** blob, item, revision, attestation. It is the hardest thing to change later.
  - [x] Draft spec: [docs/spec/content-model.md](docs/spec/content-model.md)
  - [x] Review the open questions and lock it as format version 1 (D-038)

## Phase 1: v0.1 "One box"

**NEVO node**

- [ ] Rust core, one binary for Linux ARM, Linux x86 and Windows
  - [x] `nevo-node`: first runnable node (`nevo` binary, 6 MB) built for Linux x86
  - [x] Cross-builds for 32-bit and 64-bit ARM Linux (5–6 MB; not yet run on a Pi)
  - [x] Windows build (8.6 MB, cross-compiled; runs on Windows 11 and passes all tests there)
  - [x] CI (GitHub Actions): format, lint and tests on Linux, Windows and macOS; MSRV check; release builds for all four targets
- [ ] Web UI: browse, read, search; works on old phones and without JavaScript
  - [x] Browse topics, read guides (safe Markdown), view and stream files, add guides with uploads
  - [ ] Search
  - [ ] Storage housekeeping: remove files no version uses (from failed or abandoned uploads), and refuse uploads when free space runs low
- [ ] Built-in AI: RAG with citations, "I don't know," answer template, request queue (off-the-shelf small model)
- [ ] Search: keyword across ZIM packs and the local library; semantic search on uploaded content
- [ ] Import from USB and Wi-Fi upload: PDF, Markdown, images, ZIM, with import safety checks
- [ ] Content model from day one: signed revisions, fingerprinted blobs
  - [x] `nevo-format` crate: format v1 objects, IDs, signatures, strict CBOR; matches the Python reference encoder
  - [x] `nevo-store` crate: object database (SQLite), verified blob files, chained sync logs with fork detection
- [ ] Roles: anonymous reader, contributor, admin
- [ ] Setup wizard

**NEVO Seed**

- [ ] Built-in catalog and kits
- [ ] Resumable, verified downloads
- [ ] Seed stick writing: exFAT, fake-capacity test, verification after writing
- [ ] Vault mode

## Phase 2: v0.2 "Trust and richness"

- [ ] Curators, attestations, the four trust labels, topic-scoped trust
- [ ] AI trust rules: ignore Unknown in safety-critical topics, flag disagreements
- [ ] Video and audio import, transcripts
- [ ] Power governor (solar-aware)
- [ ] Seed: pre-digest, Build a pack, website capture (WARC), updates from a node's inventory
- [ ] Private and node-only content
- [ ] macOS build

## Phase 3: v0.3 "Spread"

- [ ] Intranet sync: discovery, QR pairing, log exchange, file sync policies
- [ ] USB `.nevo` bundles for node-to-node sync
- [ ] Community catalogs
- [ ] Version branches and merges in the UI

## Phase 4: v1.0 "Ready for the world"

- [ ] The fine-tuned NEVO model for each tier
- [ ] Precomputed summary card and procedure packs (water, wounds, fire, food, shelter)
- [ ] Ready-to-flash images from Seed
- [ ] Interface translations, starting with French and Spanish
- [ ] Printable bootstrap guide
- [ ] Security review
- [ ] Documentation for non-technical users

## After v1.0

- Bulletin board: needs, offers, alerts, announcements, with expiry dates
- Android app
- Voice input and output (Whisper, Piper) and offline translation of content (Argos Translate)
- LoRa / Meshtastic headlines ("new verified guide available")
- Encryption for private content
