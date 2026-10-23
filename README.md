# NEVO: NaturalEvolution

**An offline library of human knowledge, with a built-in AI, that runs on a small solar-powered computer.**

[![CI](https://github.com/ChristianBosse/NaturalEvolution/actions/workflows/ci.yml/badge.svg)](https://github.com/ChristianBosse/NaturalEvolution/actions/workflows/ci.yml)

> **Status: early development (Phase 1).** A first node runs: you can browse, search, read, add and edit guides from any browser. The AI, ZIM packs, sync and USB bundles are built as libraries and being connected. Nothing has run on a real Raspberry Pi yet. See [what works today](#status-and-trying-it).

## What is NEVO?

NEVO turns a cheap, low-power computer (such as a Raspberry Pi with 4 GB of RAM) into a local knowledge hub that keeps working when the internet doesn't.

- **A huge offline library.** Wikipedia, medical references, farming and repair guides, books, maps, videos and documents, all stored on the device.
- **A built-in AI assistant.** Ask "how do I treat a burn?" and get clear, step-by-step answers drawn **only from the library**, with links to the sources. When the library doesn't cover something, it says so.
- **Anyone can use it.** The node creates its own Wi-Fi. Any phone, tablet or laptop with a browser can join and open `nevo.local`. Nothing to install.
- **The community adds its own knowledge.** People upload guides, photos, videos and documents. The AI can use them within minutes.
- **Nodes share knowledge with each other** over a local network or a USB stick, without the internet. Knowledge spreads from village to village like seeds.
- **Strictly offline.** NEVO never connects to the internet. Nothing phones home.

## Who is it for?

Anyone who needs knowledge without a reliable internet connection: off-grid communities, schools in remote regions, disaster relief, preppers, and anyone planning for the day the network goes down.

## The two programs

| Program | Runs on | Job |
|---|---|---|
| **NEVO node** | The offline device (Pi, old laptop, PC) | Library, search, AI assistant, uploads, node-to-node sync |
| **NEVO Seed** | Any computer with internet | Downloads recommended content, prepares it, and writes a ready-to-import USB "seed" stick |

NEVO itself never goes online. Seed prepares content while internet exists; the node uses it forever after.

## Hardware

| Tier | RAM | What you get |
|---|---|---|
| Standard | 4 GB (Linux) | Every feature, simpler AI answers |
| Enhanced | 8 GB | Smarter, longer AI answers |
| Power | 16 GB+ | Best answers, faster processing |

Every node runs the AI itself. On Windows, plan for about 8 GB, since Windows uses more memory than Linux.

Planned platforms: Linux (ARM and x86), Windows, macOS. Android later.

## How it's built

- **Rust** core, shipped as a single program per platform (Rust 1.88 or newer)
- **llama.cpp** compiled in for the AI, running small open models (Qwen3.5, Gemma 4)
- **Kiwix ZIM** files for large reference libraries
- **Tantivy** for search, **SQLite** for data
- Every piece of content is signed and identified by its hash ([content format](docs/spec/content-model.md)), so it can be checked without trusting whoever passed it on
- Plain, open file formats, so the knowledge outlives the software

The code is a Rust workspace in [`crates/`](crates):

| Crate | What it does |
|---|---|
| `nevo-node` | The `nevo` program: the web server people use |
| `nevo-format` | The content format: IDs, signatures, objects |
| `nevo-store` | Storage: objects, files, sync logs, integrity checks |
| `nevo-search` | Multilingual search, with passages for the AI |
| `nevo-trust` | Trust labels: Reference, Verified, Community, Unknown |
| `nevo-llm` | Runs the AI model with llama.cpp |
| `nevo-answer` | The AI's prompt, and code checks on its answers before anyone sees them |
| `nevo-zim` | Reads Kiwix ZIM packs and extracts clean text |
| `nevo-extract` | Checks uploaded files and extracts text from PDF, EPUB and web pages |
| `nevo-sync` | Sync between nodes on a local network |
| `nevo-bundle` | `.nevo` bundles for USB sticks, and export to plain folders |
| `nevo-seed` | NEVO Seed tools: verified downloads, USB stick test, vault checks |

## Status and trying it

NEVO is in Phase 1 ([roadmap](ROADMAP.md)).

**Works today in the `nevo` program:**

- A setup wizard on first start (it needs a one-time code printed in the console), with accounts for contributors and admins
- Browsing by topic, reading guides (with photos, video and audio), and search across guides and the text of uploaded PDFs, EPUBs and web pages
- Adding and editing guides; when two people edit offline, comparing and merging their versions; full history
- Trust labels on every item, with the reasons behind them
- Private items only their author sees, and items that never leave this node
- Housekeeping: unused files are cleaned up and uploads stop before the disk is full

**Built and tested, not yet connected to the web pages:** the AI (model runner plus answer safety checks), Kiwix ZIM packs, sync between nodes, USB bundles, and the NEVO Seed tools.

**Not done yet:** a test on a real Raspberry Pi, ready-made downloads, and the Seed program itself.

To run it with Docker (no Rust install needed), from the repository folder:

```bash
docker run --rm -it -p 8080:8080 -v "$PWD:/src" -w /src rust:1-bookworm cargo run --release -p nevo-node -- --data /tmp/nevo
```

Then open http://localhost:8080 (or `http://<this computer's address>:8080` from a phone on the same Wi-Fi). On first start the console prints a setup code: the setup page asks for it before creating the first admin account.

CI builds and tests every change on Linux, Windows and macOS, and builds the program for 32-bit and 64-bit Raspberry Pi.

## Documents

- [VISION.md](VISION.md): why NEVO exists, its principles and its design
- [ROADMAP.md](ROADMAP.md): phases from first prototype to v1.0 and beyond
- [DECISIONS.md](DECISIONS.md): every major decision so far and the reasoning behind it ("Proposed" ones still need review)
- [Content format](docs/spec/content-model.md), [sync protocol](docs/spec/sync.md) and [catalog format](docs/spec/catalog.md): the specs
- [Security](docs/security.md): the threat model, and what is protected and what isn't yet
- [Kits](docs/kits.md): what a Seed stick can carry, per topic, language and stick size

### Guides

Printable guides for people who run and use a node:

- [Setting up a NEVO node](docs/guide/setup.md): hardware, power, installing, Wi-Fi hotspot, starting at boot, backups
- [Using NEVO](docs/guide/using.md): browsing, reading and adding guides
- [Looking after a NEVO](docs/guide/admin.md): disk space, updates, moving to new hardware, recovery

## Contributing

NEVO is in early development. If you care about offline knowledge, open an issue or a discussion. To report a security problem, see [SECURITY.md](SECURITY.md). Not all help is code: curating knowledge packs (medical, farming, repair) and translating will matter as much as programming. See [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

NEVO's code is released under the **GNU AGPL-3.0** (see [LICENSE](LICENSE)). Content keeps its own licenses: community contributions default to **CC BY-SA 4.0**, and each knowledge pack records its own license.
