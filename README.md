# NEVO: NaturalEvolution

**An offline library of human knowledge, with a built-in AI, that runs on a small solar-powered computer.**

> **Status: planning.** There is no code yet. This repository currently holds the vision, the roadmap and the design decisions. See [ROADMAP.md](ROADMAP.md) for what comes first.

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

## How it's built (planned)

- **Rust** core, shipped as a single program per platform
- **llama.cpp** compiled in for the AI
- **Kiwix ZIM** files for large reference libraries
- **Tantivy** for search, **SQLite** for data
- Plain, open file formats, so the knowledge outlives the software

## Documents

- [VISION.md](VISION.md): why NEVO exists, its principles and its design
- [ROADMAP.md](ROADMAP.md): phases from first prototype to v1.0 and beyond
- [DECISIONS.md](DECISIONS.md): every major decision so far and the reasoning behind it

## Contributing

NEVO is in the planning phase. If you care about offline knowledge, open an issue or a discussion. Not all help is code: curating knowledge packs (medical, farming, repair) and translating will matter as much as programming. See [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

NEVO's code is released under the **GNU AGPL-3.0** (see [LICENSE](LICENSE)). Content keeps its own licenses: community contributions default to **CC BY-SA 4.0**, and each knowledge pack records its own license.
