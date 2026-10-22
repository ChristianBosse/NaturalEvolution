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

NEVO is a Rust workspace (Rust 1.85 or newer). The code lives in `crates/`:

| Crate | What it does |
|---|---|
| `nevo-format` | The content format ([spec](docs/spec/content-model.md)): IDs, signatures, objects |

With Rust installed:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Without installing Rust, use Docker:

```bash
docker run --rm -v "$PWD:/src" -w /src rust:1-bookworm cargo test
```

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
