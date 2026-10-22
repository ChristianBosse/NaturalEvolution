# ZIM reading and search results (Phase 0)

*Run 2026-10-22 with Rust 1.99.1, `zim` 0.5.0 and Tantivy 0.26.2. Every "node" run is in a Docker container capped at 4 GB RAM and 4 cores of a Ryzen 7 9800X3D.*

**Question:** can NEVO read and search Kiwix ZIM files from Rust on a 4 GB node?

**Short answer:** yes. Reading is instant, and indexing a 1 GB medical encyclopedia takes 35 seconds on one core. Search takes under 1 ms. Ranking needs work, but the fixes are known.

## Test files

All from [download.kiwix.org](https://download.kiwix.org/zim/wikipedia/), checked against Kiwix's SHA-256 sums, and all in ZIM format 6.3, the current one.

| File | Size | Articles | Redirects | Text |
|---|---|---|---|---|
| Ray Charles (maxi) | 3 MB | 340 entries | | small sanity check |
| WikiMed, mini (introductions only) | 174 MB | 72,900 | 267,399 | 108 MB |
| WikiMed, nopic (full articles, no images) | 1,048 MB | 104,216 | 267,399 | 999 MB |

## 1. Reading

The pure-Rust [`zim`](https://crates.io/crates/zim) crate read every file with no problems: metadata, the main page, the article list, redirects, and both compression formats.

- **Opening a file takes under 1 ms and about 10 MB**, because the file is memory-mapped, not loaded.
- **Reading one article takes about 2 ms**, including decompression.
- Redirects work: `Heart_attack` opens *Myocardial infarction*.

## 2. Indexing

NEVO can't use the ZIM's built-in search index, because it is in Xapian's C++ format and has no Rust reader. It builds its own Tantivy index instead. The index stores only each article's path and title, plus every redirect as an alias. Article text stays in the ZIM and is read back when needed.

| File | Who builds it | Time | Peak heap memory | Index size |
|---|---|---|---|---|
| WikiMed mini, 174 MB | 4 GB node, 1 thread, 50 MB writer | **5 s** | under 0.23 GB total | 50 MB (29%) |
| WikiMed full text, 1 GB | 4 GB node, 1 thread, 50 MB writer | **35 s** | 0.28 GB | 353 MB (34%) |
| WikiMed full text, 1 GB | Seed, 8 threads, 800 MB writer | **18 s** | 1.7 GB total | 346 MB (33%) |

- **The index is about a third of the ZIM's size**, or about a third of the plain text. Full English Wikipedia would need an index of several GB. Keyword search for it is feasible; semantic search over it is not (D-015 already expected this).
- **Process memory looks larger than it is.** The node run showed 1.1 GB, but only 0.28 GB was real heap memory. The rest was the memory-mapped ZIM, which the system can drop at any time.
- **A Pi will be slower.** A desktop core is several times faster than a Pi core, so expect minutes instead of seconds. Still fine, and Seed can do it ahead of time (D-029).
- **Memory clash on 4 GB nodes:** the AI model uses 3.4 GB (D-031). Indexing (0.3 GB) and the model can't run at the same time on Standard nodes, so indexing has to wait until the model is unloaded.

## 3. Search speed

| Step | Time on the node |
|---|---|
| Open the index | under 1 ms |
| Search, top 10 | 0.3–1 ms |
| Read the 10 articles from the ZIM and build snippets | 7 ms (intros), 15–23 ms (full articles) |

Memory use while searching was about 25 MB.

## 4. Search quality

The first version used simple English word matching (with stemming) and ranked several obvious queries badly. Two cheap fixes helped a lot:

1. **Drop stop words** ("how", "to", "the", "in").
2. **Exact title or redirect match goes first.** Typing "heart attack" now finds *Myocardial infarction* first instead of *Panic attack*.

| Query | Top result after the fixes | Good? |
|---|---|---|
| heart attack | Myocardial infarction | yes (was *Panic attack*) |
| oral rehydration solution | Oral rehydration therapy | yes |
| how to stop severe bleeding | Bleeding | yes |
| hypothermia treatment | Hypothermia | yes |
| dehydration in children | Dehydration | yes |
| diarhea (typo) | Diarrhea | only because Wikipedia has that redirect |
| snake bite first aid | First aid; *Snakebite* is #6 | partly |
| morsure de serpent (French) | junk | no |

What's still missing:

- **A reranker.** "Snake bite" doesn't match the one-word title *Snakebite*. The article is in the top 10, so the AI would see it, but a small reranking model would put it first (planned in D-015).
- **Questions in another language than the pack.** A French question on an English pack finds nothing useful. The built-in AI can translate the question into the pack's language before searching.
- **Typo tolerance.** Tantivy has fuzzy matching; it wasn't tested here.
- **A real text extractor.** The crude HTML stripper keeps reference lists, which show up in snippets ("Retrieved 24 August 2009…"). NEVO's extractor must skip references and navigation boxes and keep headings, lists and tables.
- **Passages for the AI.** This spike indexes whole articles. The AI needs short passages (a section or a few paragraphs), so the real index also needs passage-level entries.

## 5. Dependencies

- `zim` 0.5.0 is MIT/Apache-2.0, pure Rust plus the standard zstd and xz C libraries, which build fine for ARM and Windows. It also pulls in command-line libraries (clap, indicatif, rayon and others) that only its bundled tools need. NEVO should get these made optional upstream, or vendor the crate (its license allows it). The reading code is about 1,900 lines.
- The spike's release binary is 9 MB with 190 crates in total, mostly from Tantivy.

## Limits of this test

- Desktop cores, not a Pi.
- English only. Other languages need their own stemmers and stop words. Tantivy has them for the major European languages, but not for every language NEVO's users speak.
- 8 hand-picked queries, no measured recall.
- ZIM files with images (maxi) weren't indexed. Images don't change the text index, only the ZIM size.
