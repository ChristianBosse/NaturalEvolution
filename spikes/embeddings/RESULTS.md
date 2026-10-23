# Embeddings and reranker results (Phase 0)

*Run 2026-10-22 with llama.cpp build b11803, all models Q8_0. Quality on the GPU; speed and memory in a Docker container capped at 4 GB RAM (no swap) and 4 cores of a Ryzen 7 9800X3D. Other agents' containers were using the same CPU at the time, so the speed numbers are on the slow side and noisy.*

**Question:** which embedding model and which reranker should NEVO use, and can a 4 GB node run them next to the 3.4 GB chat model?

**Short answer:** use **multilingual-e5-small** (MIT, 132 MB) as the one embedding model for Seed and every node, fused with keyword search. **No reranker on the 4 GB tier**: the good ones take 30–40 s per question on 4 desktop cores, so minutes on a Pi, and 0.85–1.2 GB of RAM. A reranker (Qwen3-Reranker-0.6B) is a later option for bigger tiers, after a Pi test.

## 1. Search quality

70 questions over 2,500 WikiMed passages (see [README.md](README.md)). Each cell is **Recall@10 / MRR**. "Translated" means the French or Spanish question was replaced by its English wording, standing in for the AI translating it first (D-037).

| Pipeline | All (70) | English (36) | Everyday words (10) | French + Spanish (24) |
|---|---|---|---|---|
| BM25 keyword search | 0.63 / 0.43 | 0.94 / 0.70 | 0.80 / 0.46 | 0.08 / 0.02 |
| BM25, question translated | 0.94 / 0.69 | 0.94 / 0.70 | 0.80 / 0.46 | 1.00 / 0.79 |
| Embeddings: multilingual-e5-small | 0.91 / 0.69 | 0.97 / 0.75 | 0.90 / 0.66 | 0.83 / 0.63 |
| Embeddings: granite-embedding-107m | 0.87 / 0.67 | 0.94 / 0.77 | 0.80 / 0.53 | 0.79 / 0.58 |
| Embeddings: nomic-embed-text-v2-moe | 0.94 / 0.71 | 0.97 / 0.79 | 0.90 / 0.58 | 0.92 / 0.64 |
| Embeddings: Qwen3-Embedding-0.6B | **1.00 / 0.79** | 1.00 / 0.82 | 1.00 / 0.82 | 1.00 / 0.75 |
| **Hybrid: BM25 translated + e5-small (RRF)** | **0.96 / 0.78** | 0.94 / 0.78 | 1.00 / 0.62 | 0.96 / 0.83 |
| Hybrid: BM25 translated + Qwen3-Embedding (RRF) | 0.96 / 0.83 | 0.94 / 0.81 | 0.90 / 0.73 | 1.00 / 0.91 |
| BM25 top 50 → bge-reranker-v2-m3 | 0.67 / 0.55 | 0.94 / 0.79 | 0.90 / 0.80 | 0.17 / 0.11 |
| BM25 translated top 50 → bge-reranker-v2-m3 (English query) | 0.96 / 0.81 | 0.94 / 0.79 | 0.90 / 0.80 | 1.00 / 0.86 |
| BM25 translated top 50 → Qwen3-Reranker-0.6B (English query) | 0.96 / **0.84** | 0.94 / 0.83 | 0.90 / 0.78 | 1.00 / 0.89 |
| e5-small top 50 → Qwen3-Reranker-0.6B | 0.99 / 0.82 | 1.00 / 0.87 | 1.00 / 0.86 | 0.96 / 0.72 |

The full table (24 pipelines) is in `results/table.txt`, per-question ranks in `results/metrics.json`.

What this shows:

- **Passage-level indexing fixes "snake bite first aid".** With passages headed "Snakebite - First aid", every pipeline, plain BM25 included, puts the right passage first. The #6 rank in the ZIM spike came from indexing whole articles (D-037 already asked for passages).
- **Keyword search fails across languages, and translation fixes it.** French and Spanish questions over English passages: BM25 finds 8%; with the English wording, 100%. "Morsure de serpent" is #30 with e5-small alone, #1 with translated BM25.
- **Embeddings help most with everyday words.** "Headache and dizzy in a closed garage with the engine running": BM25 #26, Qwen3-Embedding #1. MRR on these 10 questions goes from 0.46 (BM25) to 0.66 (e5-small) and 0.82 (Qwen3).
- **The cheap hybrid gets most of the gain.** BM25 + e5-small fused with reciprocal rank fusion (RRF, no extra model) reaches 0.96 / 0.78, close to the best single model (Qwen3-Embedding, 1.00 / 0.79) and to BM25 + a reranker (0.96 / 0.84).
- **Rerankers raise MRR by about 0.1–0.15** over BM25 on English, mostly on everyday wording. They need the translated (English) question: with the French or Spanish question they do worse than translated BM25 alone (0.76 vs 0.79).
- **"What not to do after a snake bite" still fails** for keyword search (all useful words are stop words) and ranks 6–7 with embeddings.
- **Smaller vectors cost nothing.** int8 vectors scored the same as float32 for every model. Qwen3-Embedding cut to its first 384 dimensions (it is trained for this) scored 1.00 / 0.81, slightly better than full size.
- Differences under about 0.05 are 1–3 questions and within noise.

## 2. Speed and memory on 4 GB

Docker, 4 cores, 4 GB, no swap. Embedding speed is 300 passages (about 145 words each) sent 16 at a time. Reranking is 10 questions, each with its BM25 top 50. "Process" is the llama-server peak RSS; "container" also counts the unpacked llama.cpp files and the model file in the page cache. Server settings: `-c 4096 -b 2048 -ub 2048 -np 4 --cache-ram 0 --no-repack`.

| Model | File | Passages/s | One query | Rerank 50 | Process peak | Container peak |
|---|---|---|---|---|---|---|
| multilingual-e5-small | 132 MB | **29.9** | **7 ms** | | **0.31 GB** | 0.49 GB |
| granite-embedding-107m | 121 MB | 32.0 | 14 ms | | 0.30 GB | 0.42 GB |
| nomic-embed-text-v2-moe | 512 MB | 3.3 | 71 ms | | 0.75 GB | 0.86 GB |
| Qwen3-Embedding-0.6B | 639 MB | 1.1 | 206 ms | | 1.20 GB | 1.34 GB |
| Qwen3-Embedding-0.6B, repacked | 639 MB | 2.9 | 95 ms | | 1.20 GB | 1.33 GB |
| bge-reranker-v2-m3 | 636 MB | | | 31 s (max 71 s) | 0.85 GB | 1.01 GB |
| Qwen3-Reranker-0.6B | 639 MB | | | 40 s (max 59 s) | 1.20 GB | 1.35 GB |

- **Repacking matters for the 0.6B models, not for e5-small.** Without `--no-repack` (D-033), Qwen3-Embedding was 2.6 times faster at the same memory; e5-small was unchanged (28.9 vs 29.9 passages/s). The reranker runs with repacking were not finished (see "Still missing").
- **Rerankers are far too slow for a Pi.** 30–40 s per question on 4 desktop cores.

### Index size per 100,000 passages

Vectors only, searched by brute force (100,000 int8 dot products take milliseconds; no extra index structure needed at this size).

| Dimensions | float32 | int8 |
|---|---|---|
| 384 (e5-small, granite, Qwen3 cut to 384) | 154 MB | **38 MB** |
| 768 (nomic v2) | 307 MB | 77 MB |
| 1024 (Qwen3-Embedding) | 410 MB | 102 MB |

For comparison, the Tantivy index for 1 GB of WikiMed text is 350 MB (D-036).

### Pi estimate

A desktop Zen 5 core is several times faster than a Pi 5 core at this kind of math. Assuming 4–8 times slower (not measured):

| Task | 4 desktop cores | Pi 5, estimated |
|---|---|---|
| e5-small: embed one question | 7 ms | under 0.1 s |
| e5-small: embed 100,000 passages | about 1 hour | 4–8 hours (Seed does it instead, D-029: 2,500 passages took 10 s on the GPU) |
| Qwen3-Embedding: embed one question | 0.1–0.2 s | about 1 s |
| Rerank 50 passages (either reranker) | 30–40 s | 2–5 minutes |
| Rerank 10 passages | about 6–8 s | 30–60 s |

## 3. Recommendation

**Embedding model: multilingual-e5-small (Q8_0, 384 dimensions, MIT), for Seed and every node.**

- It is the only good model small enough to stay loaded next to the chat model on 4 GB: 132 MB file, 0.31 GB peak with the test's large buffers (less with one 512-token slot; not measured yet).
- Quality alone is fair (0.91 / 0.69); fused with BM25 it is close to the best model (0.96 / 0.78).
- A question embeds in milliseconds even on a Pi. Seed embeds packs (D-029).
- Prefixes matter: `query: ` for questions, `passage: ` for passages.
- Store vectors as int8: 38 MB per 100,000 passages.
- Use the GGUF from `TwinSunsLLC/multilingual-e5-small-gguf`. Two other conversions were broken: one doesn't load in current llama.cpp, the other loads but gives nearly the same score to every passage. NEVO should convert the model itself and check it with a fixed test like `sanity.py`.

**Search pipeline:** passage-level BM25 and e5-small, fused with RRF. When the question's language differs from the pack's, the chat AI translates the question for the BM25 side (D-037); the embedding side uses the original question.

**Reranker: none on the 4 GB tier.** Too slow (minutes per question on a Pi) and too big to sit next to the chat model (0.85–1.2 GB). For 8 GB and up, **Qwen3-Reranker-0.6B (Apache-2.0)** gave the best MRR (0.84), with **bge-reranker-v2-m3 (Apache-2.0)** close behind (0.81). Only after a Pi test, and on the top 10–20 passages, not 50.

**The bigger embedding model is the upgrade path, not the default.** Qwen3-Embedding-0.6B is clearly better on everyday wording and other languages (1.00 / 0.79), and can be cut to 384 dimensions with no loss. But it needs 1.2 GB, so it can't share 4 GB with the chat model, and D-029 requires one model for all nodes. Revisit if the Standard tier moves to 8 GB or the chat model shrinks.

### Memory budget on a 4 GB node

| Item | Memory |
|---|---|
| Chat model, Qwen3.5 4B (D-031) | 3.4 GB peak |
| e5-small embedder, kept loaded | about 0.2–0.3 GB |
| Search (Tantivy and vectors, memory-mapped) | about 0.03 GB plus page cache |
| Left for the system and NEVO | about 0.3 GB: tight |

Swapping models per question isn't an option: reloading the chat model took 27 s on the desktop (D-031), longer from an SD card. So on 4 GB, the embedder stays loaded next to the chat model, with one 512-token slot. If that is too tight on a real Pi, the fallbacks are, in order: the 3072-token chat context suggested in the AI benchmark, the Qwen3.5 2B fallback model (1.7 GB), or keyword search plus translation only (0.94 / 0.69 here). Indexing and embedding new content on the node happen only while the chat model is unloaded (D-036).

## 4. Licenses

| Model | License | Use |
|---|---|---|
| multilingual-e5-small | MIT | recommended |
| granite-embedding-107m-multilingual | Apache-2.0 | OK, slightly worse |
| nomic-embed-text-v2-moe | Apache-2.0 | OK, slow on CPU |
| Qwen3-Embedding-0.6B | Apache-2.0 | upgrade path |
| Qwen3-Reranker-0.6B | Apache-2.0 | reranker for 8 GB+ |
| bge-reranker-v2-m3 | Apache-2.0 | reranker fallback |
| gte-multilingual-reranker-base | Apache-2.0 | doesn't load in llama.cpp b11803 |
| jina-reranker-v2-base-multilingual, jina-reranker-v3 | **CC-BY-NC-4.0** | excluded: non-commercial |
| EmbeddingGemma 300m | Gemma terms | excluded: not Apache/MIT |

## 5. Limits of this test

- 70 questions written by one author who had seen the articles, so English keyword wording is easy (BM25 already finds 94%). The 10 everyday-wording questions are closer to real use, but few.
- English passages only. French and Spanish packs (questions and passages in the same language) weren't tested.
- Relevant passages are marked by rules (article, words, sections), not read one by one.
- Desktop CPU shared with other agents, not a Pi. Pi numbers are estimates.
- Machine translation is simulated with hand-written English questions; real AI translations will be a bit worse.

## Still missing

- **Reranker speed with repacking** (`NEVO_REPACK=1 python run_emb.py --env cpu4 --only bge-rerank-m3 qwen3-rerank-0.6b`). The run was started but not finished when this was written.
- **e5-small with node settings** (`NEVO_SMALL=1 python run_emb.py --env cpu4 --only e5-small`): one 512-token slot, to confirm the 0.2–0.3 GB estimate in the memory budget.
- **e5-small and the chat model in one 4 GB container**, both loaded, answering a question.
- **Everything on a real Pi.**
