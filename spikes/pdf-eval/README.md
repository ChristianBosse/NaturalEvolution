# Spike: pure-Rust PDF text extraction

Throwaway comparison for `crates/nevo-extract`. Which maintained, pure-Rust crate should pull text out of uploaded PDFs?

Candidates (all older than 14 days on 2026-10-22):

- [`pdf-extract`](https://crates.io/crates/pdf-extract) 0.12.1: text extraction on top of lopdf, with font decoding (ToUnicode maps, standard encodings, CFF and Type 1 fonts) and character positions.
- [`lopdf`](https://crates.io/crates/lopdf) 0.42.0 `Document::extract_text`: the parser's own simple extractor.
- [`pdf`](https://crates.io/crates/pdf) 0.10.0 (pdf-rs): a parser only. It has no text extraction, so using it means writing font decoding ourselves. Not measured.

Quality is the share of the source text's words found in the output, in order (longest common subsequence over words).

## How to run

```bash
docker run --rm -v "$PWD:/src" -v "<folder with PDFs>:/data" -v nevo-cargo:/usr/local/cargo/registry \
  -v nevo-target:/target -e CARGO_TARGET_DIR=/target/pdf-eval -w /src/spikes/pdf-eval nevo-rust \
  bash -c "cargo build --release && /target/pdf-eval/release/pdf-eval /data/book.pdf /data/book.txt"
```

## Results (desktop, one core, release build)

**Book, 62 pages, 383 KB.** NEVO's own docs (VISION, ROADMAP, DECISIONS, the content-model spec; AGPL) three times over, as HTML printed to PDF by headless Microsoft Edge: embedded subset TrueType fonts, compressed streams, the usual real-world shape.

| Extractor | Time | Peak heap | Words recalled in order | Paragraph breaks |
|---|---|---|---|---|
| lopdf `extract_text` | 0.029 s | 3.6 MB | 100.0% | no (one line per text run) |
| pdf-extract | 0.058 s | 3.8 MB | 100.0% | yes (blank line at wide vertical gaps) |

**License agreement, 6 pages, 385 KB**, made by Microsoft Word (local file, not committed). lopdf split words in the middle ("m odifications", "a have"); pdf-extract read them whole.

**Choice: pdf-extract.** Same accuracy on clean files, better on Word output, and its `OutputDev` callback gives each character's position and font size, which nevo-extract uses for paragraphs, headings, a cooperative deadline and a text limit. Its weakness is many `unwrap`s on malformed input; nevo-extract catches those panics per page and checks the two unbounded recursions (page `Parent` loops, self-drawing XObjects) before drawing.

Through nevo-extract (paragraphs, headings, passages), release build:

| File | Time | Peak heap | Text | Passages |
|---|---|---|---|---|
| 50-page generated PDF (Helvetica, 241 KB) | 0.029 s | 1.1 MB | 161,000 chars | 150 |
| The 62-page Edge book above | 0.050–0.062 s | 3.6 MB | 122,252 chars | 192 |

The Edge book came out with all `h1` and `h2` headings found by font size; `h3` (13 pt over 12 pt body) is too close to body text to tell apart.
