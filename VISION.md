# NEVO Vision

## Why NEVO exists

Most of humanity's practical knowledge now lives online. If the network goes down (through disaster, conflict, collapse, or simply because a place was never connected), that knowledge becomes unreachable exactly when people need it most: how to purify water, treat a wound, grow food, repair a pump, wire a solar panel.

NEVO keeps that knowledge **local, searchable, explainable and shareable**, on hardware cheap and frugal enough to run from a small solar panel for years.

## Principles

These settle future arguments. When a feature conflicts with a principle, the principle wins.

1. **Offline first, offline only.** NEVO never connects to the internet. Every feature works with no connection at all.
2. **Every node is complete.** Every node runs its own AI, search and library. There are no thin clients or relay nodes. Minimum hardware is about 4 GB of RAM.
3. **Every answer has a source.** The AI answers only from the library and cites what it used. If the library doesn't cover it, the AI says "I don't know."
4. **Search always works.** The AI builds on search; it never replaces it.
5. **Durable, open formats.** Everything can be read in 50 years without NEVO: Markdown, JSON or CBOR, ordinary files, and standard archive formats.
6. **Self-contained.** NEVO carries the knowledge needed to rebuild NEVO, including a printable setup guide.
7. **Each node is sovereign.** Each node decides what it stores, whom it trusts and whom it syncs with.
8. **Open source.** The code is AGPL-3.0, so improvements stay available to everyone.

## Non-goals

- Internet sync, cloud features, accounts on a central server
- Chat or direct messaging (a small bulletin board is planned for v2)
- Running on devices with less than about 4 GB of RAM
- Training a language model from scratch

## The system at a glance

```
 ONLINE COMPUTER (while internet exists)
 ┌──────────────────────────────────────────────┐
 │ NEVO Seed: catalogs, kits, website capture,  │
 │ pre-digest, build a pack, vault mode         │
 └──────────────────────┬───────────────────────┘
                        │ USB seed stick
                        ▼
 NEVO NODE (strictly offline)          ◄── browsers on Wi-Fi
 ┌──────────────────────────────────────────────┐
 │ Web UI · Import queue · Search · Built-in AI │
 │ Sync engine · Trust · Power governor · Setup │
 │──────────────────────────────────────────────│
 │ Storage: ZIM packs · Blob store · SQLite     │
 └──────────────────────┬───────────────────────┘
                        │ intranet or USB bundles
                        ▼
                 Other NEVO nodes
```

## The NEVO node

### Clients

Any device with a web browser joins the node's Wi-Fi and opens `nevo.local` (or is sent there by a captive portal). Pages are built on the node, so they work on old phones and with JavaScript turned off. Phones can optionally save articles for offline reading.

### Roles

| Role | Can |
|---|---|
| Reader (no account) | Browse, search, ask the AI |
| Contributor | Upload, write and edit guides |
| Curator | Verify or flag content in their topics |
| Admin | USB imports, AI model, trust settings, sync pairing, purge content |

### Built-in AI

The AI is part of every node, never optional. It uses **retrieval-augmented generation (RAG)**:

1. A user asks a question.
2. NEVO searches the library and a reranker picks the best few passages.
3. A small local model writes an answer **using only those passages**.
4. The answer cites its sources, with their trust labels.

Answers to "what do I do" questions follow a fixed template: urgent warnings first, numbered steps, what you need, then sources.

The model size follows the hardware: roughly 1.5–2B parameters on 4 GB, about 4B on 8 GB, and 8B or more on 16 GB+, all 4-bit quantized and run by llama.cpp. On 4 GB, one answer is generated at a time, with a queue.

How a small model gets good:

- **Strong retrieval:** keyword plus meaning-based search, then a reranker
- **A fine-tuned "NEVO model":** a small open model fine-tuned to cite sources, give clear steps and refuse to guess
- **Precomputed summary cards** for critical topics (water, wounds, fire, food, shelter)
- **An answer cache** for repeated questions
- **Trust-aware answers:** in safety-critical topics (medical, electrical, chemicals, food safety), content labeled Unknown is ignored, and disagreements between sources are pointed out

New community uploads become usable by the AI within minutes. No retraining needed.

### Search

One search box covers everything: Kiwix ZIM packs and NEVO's own index. Semantic search covers curated packs and community content; very large packs such as full Wikipedia use keyword search plus the reranker, because their embeddings would not fit on small hardware. Results held by other nodes are shown too ("available at Hilltop").

### Import

Two ways in: **USB** (for big content and initial setup) and **Wi-Fi upload** from the browser (for everyday contributions). Both feed one import queue with visible progress.

Supported: ZIM, PDF, EPUB, Word/ODT, Markdown, HTML, images, video, audio, OpenStreetMap files, saved websites (WARC), GGUF AI models, `.nevo` packs.

Import safety: file types are checked by content, nothing is ever executed, parsing is isolated, quotas apply, duplicates are detected, and location data is stripped from photos.

### Power governor

The node reads battery and solar charge and adapts:

| State | Behavior |
|---|---|
| Full sun | Heavy jobs run (indexing, transcripts, embeddings) |
| Normal | Serve content, AI available |
| Low battery | AI off, jobs paused, content and search only |
| Critical | Clean shutdown to avoid corruption |

### Setup wizard

Language, admin account, hardware check and model choice, import from the seed stick, priority topics, node name and Wi-Fi hotspot, then a QR code to join.

## Content model

| Building block | What it is |
|---|---|
| **Blob** | Raw bytes stored under their BLAKE3 hash. Deduplicated, corruption-checked, split into ~1 MB chunks. |
| **Revision** | An immutable, signed version of something. Its ID is the hash of its content. Parents form a version tree. |
| **Item** | The stable "thing" (e.g. "How to build a rocket stove"), defined by its history of revisions |
| **Attestation** | A signed statement about content: verified, dangerous, outdated, translation of, hide |
| **Derived data** | OCR text, transcripts, embeddings, thumbnails. Recomputable, synced as a signed cache. |

Details:

- **Edits never overwrite.** Offline edits create branches; anyone can merge them. By default, the version with the highest trust is shown, then the most recent.
- **Deletion:** nobody can delete other people's content across nodes. They can attest "hide" or "dangerous." Each admin can purge files from their own node.
- **Time:** ordering uses logical clocks, because a node's clock can't be trusted offline. An optional GPS module gives true time.
- **Visibility:** public, node-only, or private. Private content never syncs; it is hidden, not encrypted, at first.
- **Export:** a dump command writes the whole library as plain folders.

## Sync

Sync is strictly offline and opt-in.

- **Transports:** local network or Wi-Fi (from one building to a village mesh to rooftop point-to-point links), plus **USB `.nevo` bundles** for places out of range
- **Pairing:** nodes discover each other; the admin approves a pairing once with a QR code or short code
- **Protocol:** each node keeps an append-only log of what it created. Nodes compare log positions and send what's missing.
- **Metadata always syncs.** Files sync by policy: topics, size, trust, pinned items.
- **Knowledge spreads by contact:** a node receives content from nodes it never met, through the ones it did.

## Trust

- Each node and each contributor has a key pair. Every revision and attestation is signed. Contributor keys can be exported as a QR code or backup file, and revoked if stolen.
- **Trust is chosen locally and scoped by topic**, e.g. trust a doctor for medical content only. Trust is never counted: a thousand unknown keys count for nothing.
- **Defaults for a new node:** it trusts only the catalogs that signed its seed. Content from paired nodes arrives as Community until the admin promotes the source.

| Label | Meaning |
|---|---|
| Reference | A signed official pack |
| Verified | Attested by a curator you trust, for that topic |
| Community | From a trusted node or author, not verified |
| Unknown | From sources you don't trust; shown with a warning |

## NEVO Seed (companion tool)

NEVO Seed runs on a computer with internet. It is the only part of the NEVO project that goes online.

**Modes**

- **New node:** prepare a stick that installs and fills a new NEVO
- **Update a node:** read a node's exported inventory and download only what's missing
- **Copy a seed:** duplicate a seed stick, with no internet needed
- **Build a pack:** turn your own files into a signed `.nevo` pack

**Features**

- **Catalogs:** signed content lists. The NEVO catalog is built in; community catalogs can be added, so there is no single authority. Sources include the Kiwix library, Hugging Face (models), Project Gutenberg, OpenStax and OpenStreetMap. Every entry shows its license.
- **Kits:** Medical, Farming and food, Repair and build, Survival basics, Education, Maps. The AI model is chosen for the target RAM. Global scope for v1.
- **Website capture:** saves openly licensed sites as WARC, crawling politely, from a pre-approved list. Other sites require the user to confirm they have the right to save them.
- **Pre-digest:** runs OCR, transcripts, embeddings and thumbnails on the fast computer, so the node gets content ready to use immediately
- **Drive handling:** formats as exFAT, tests real capacity (to catch fake sticks), verifies after writing, resumes interrupted writes
- **Vault mode:** marks long-term storage drives, reminds you to re-verify about yearly because unpowered flash fades, and recommends two copies on different media plus the printed guide
- **Downloads:** resumable, with mirrors and torrents and bandwidth scheduling. No accounts and no telemetry.

**Seed stick layout**

```
NEVO-SEED/
  manifest     signed list of everything, with checksums
  software/    NEVO for each platform
  models/      AI models
  packs/       knowledge packs
  docs/        printable setup guide
```

## Licensing

| Layer | License |
|---|---|
| NEVO code | AGPL-3.0 |
| Community contributions | CC BY-SA 4.0 by default |
| Knowledge packs | Each keeps its own license, recorded in its metadata |
| AI models | Prefer permissive licenses (Apache 2.0, MIT) so they can be bundled |
