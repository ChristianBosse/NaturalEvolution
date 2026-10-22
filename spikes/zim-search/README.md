# Spike: ZIM reading and search (Phase 0)

Throwaway experiment for the roadmap item **"read and search Kiwix ZIM files from Rust"**.

This is not NEVO code. It answers three questions:

1. Can pure Rust read real Kiwix ZIM files: articles, redirects, metadata?
2. How long does it take to build a [Tantivy](https://github.com/quickwit-oss/tantivy) search index from a ZIM, and how big is it?
3. How fast is a search, including reading the matching text back out of the ZIM for snippets?

## Why not use the ZIM's own search index?

Kiwix ZIM files include a full-text index, but it is in Xapian's format (C++). There is no Rust reader for it. NEVO needs one search index across ZIM packs, community content and PDFs, so it builds its own Tantivy index. The index stores only paths and titles; article text is read from the ZIM when needed.

## How it works

- Reads ZIMs with the [`zim`](https://crates.io/crates/zim) crate (pure Rust, MIT/Apache-2.0).
- Walks the content once, groups articles by compressed cluster (so each cluster is decompressed once), and turns redirects into aliases. For example, the "Heart attack" redirect becomes an alias of "Myocardial infarction", so searching either finds the article.
- Strips HTML with a crude built-in converter.
- English stemming (`en_stem`); title matches count 3×, aliases 2×.

## How to run

Uses Docker only; nothing is installed on the host. The test ZIMs come from [download.kiwix.org](https://download.kiwix.org/zim/wikipedia/) and live in the gitignored `.local/zim/`.

```bash
# build (named volumes keep the crates and build output between runs)
docker run --rm -v "$PWD:/src" -v nevo-cargo:/usr/local/cargo/registry -v nevo-target:/target \
  -e CARGO_TARGET_DIR=/target -w /src rust:1-bookworm cargo build --release

# copy test files into a Docker volume (bind mounts from Windows are slow)
docker run --rm -v "<repo>/.local/zim:/in:ro" -v nevo-data:/data rust:1-bookworm cp /in/*.zim /data/

# run like a 4 GB node
docker run --rm --memory 4g --memory-swap 4g --cpus 4 -v nevo-target:/target -v nevo-data:/data rust:1-bookworm \
  /target/release/nevo-zim-spike index /data/<file>.zim /data/idx 50 1
```

Commands: `info <zim>`, `get <zim> <path>`, `index <zim> <dir> [heap-mb] [threads]`, `search <dir> <query>`.

Findings are in [RESULTS.md](RESULTS.md).
