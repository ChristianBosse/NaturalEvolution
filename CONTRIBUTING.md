# Contributing to NEVO

Thanks for your interest in NEVO. The project is in its **planning phase**: there is no code yet, so the most useful help right now is ideas, research and feedback on the design.

Start by reading [VISION.md](VISION.md), [ROADMAP.md](ROADMAP.md) and [DECISIONS.md](DECISIONS.md).

## Ways to help

You don't need to be a programmer.

| Area | Examples |
|---|---|
| **Design feedback** | Challenge a decision, point out a risk, suggest a simpler approach |
| **Research** | Benchmarks of small AI models on low-end hardware, open-licensed content sources, offline tools worth reusing |
| **Knowledge curation** | Find and review openly licensed material for kits: medical, water, food, farming, repair, energy, education |
| **Translation** | French, Spanish and other languages (interface and docs) |
| **Hardware testing** | Run NEVO on Raspberry Pis, old laptops and solar setups once code exists |
| **Code** | Rust core, search, AI integration, sync and NEVO Seed, once Phase 0 is done |

## How to contribute

- **Ideas and questions:** open a GitHub issue or discussion.
- **Disagreeing with a decision:** open an issue that references its number (for example `D-013`) and explains what you would change and why. Accepted changes get a new entry in [DECISIONS.md](DECISIONS.md) that supersedes the old one; past entries are never rewritten.
- **Changes to docs:** open a pull request. Keep it focused on one topic.

## Building and testing

NEVO is a Rust workspace (Rust 1.86 or newer). The code lives in `crates/`:

| Crate | What it does |
|---|---|
| `nevo-format` | The content format ([spec](docs/spec/content-model.md)): IDs, signatures, objects |
| `nevo-store` | Node storage: objects in SQLite, blob files, sync logs |
| `nevo-trust` | Trust labels (Reference, Verified, Community, Unknown) from the admin's choices and signed attestations |
| `nevo-answer` | AI answers without the model: builds the prompt and checks the answer before it is shown |
| `nevo-node` | The `nevo` program: web server for browsing, reading and adding guides |
| `nevo-bundle` | `.nevo` bundles for USB transfer, and plain-folder export (`nevo-bundle` program) |
| `nevo-search` | Search: a Tantivy index of library items and ZIM articles, with passages for the AI |
| `nevo-seed` | The `nevo-seed` program: verified resumable downloads, USB stick capacity test, vault checks |
| `nevo-sync` | Sync between paired nodes on the local network ([spec](docs/spec/sync.md)): server routes, client, pull rounds |
| `nevo-zim` | Kiwix ZIM packs: metadata, entries, articles for indexing, clean text and passages |

With Rust installed:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Without installing Rust, use Docker:

```bash
docker run --rm -v "$PWD:/src" -w /src rust:1-bookworm cargo test
```

### CI

GitHub Actions ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs on every push and pull request to `main`:

- format, clippy and tests on Linux, Windows and macOS (Rust 1.99.1);
- `cargo check` on Rust 1.86, our minimum version. The workspace uses resolver 3, so `cargo update` only picks dependency versions that support 1.86;
- release builds of `nevo` for Linux x86_64, ARM 32-bit, ARM 64-bit and Windows x86_64. The binaries are kept as workflow artifacts for 14 days.

Actions are pinned to a full commit SHA. When you update one, use a release that is at least 14 days old.

### Cross-builds

From an x86_64 Debian or Ubuntu machine (or the `rust:1-bookworm` Docker image):

```bash
# Raspberry Pi, 32-bit and 64-bit
apt-get install gcc-arm-linux-gnueabihf gcc-aarch64-linux-gnu
rustup target add armv7-unknown-linux-gnueabihf aarch64-unknown-linux-gnu
export CC_armv7_unknown_linux_gnueabihf=arm-linux-gnueabihf-gcc
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc
export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
cargo build --release -p nevo-node --target armv7-unknown-linux-gnueabihf
cargo build --release -p nevo-node --target aarch64-unknown-linux-gnu

# Windows x86_64
apt-get install gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release -p nevo-node --target x86_64-pc-windows-gnu
```

The program is at `target/<target>/release/nevo` (`nevo.exe` on Windows). Install the cross compilers with their recommended packages: those include the target's C headers, which the bundled SQLite needs.

`spikes/` holds throwaway Phase 0 experiments. They aren't part of the workspace.

## Principles to respect

Contributions should fit NEVO's principles (see [VISION.md](VISION.md)). In particular:

- Nothing may require the internet.
- Every feature must work on a 4 GB RAM node.
- AI answers must cite the library and never invent facts.
- Data must stay in open, durable formats.

## Content licensing

Only contribute content you have the right to share. Community content is licensed **CC BY-SA 4.0** by default. Knowledge packs must record their source and license.

## Code license

By contributing code, you agree that it is released under the project's [AGPL-3.0 license](LICENSE).

## Conduct

Everyone taking part is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
