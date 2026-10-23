# Spike: embeddings and reranker (Phase 0)

Throwaway experiment to pick NEVO's embedding model and reranker: **which small, openly licensed models find the right passage for questions in English, French and Spanish, and can a 4 GB node run them?**

This is not NEVO code. It's a test harness. Findings are in [RESULTS.md](RESULTS.md).

## What it measures

| Question | How |
|---|---|
| Does it find the right passage? | 70 questions over 2,500 WikiMed passages; Recall@10 and MRR for keyword search (BM25), embeddings, hybrids and rerankers. Run on the GPU (ranking doesn't depend on the hardware). |
| Does it fit in 4 GB? | Docker container capped at 4 GB RAM (no swap) and 4 CPU cores; peak memory recorded |
| Is it fast enough? | Passages embedded per second, query time, and time to rerank 50 passages on 4 desktop cores. A Pi is several times slower. |

## Test set

- **Passages** ([extract.py](extract.py)): 2,500 passages of up to about 220 words, cut by section from the full-text WikiMed ZIM (`wikipedia_en_medicine_nopic_2026-10.zim`). 2,258 come from the 79 articles in [articles.txt](articles.txt) (every passage of each article, so the right passage competes with its own siblings), and 242 from random articles. Each passage starts with "Title - Section". References, tables and navigation boxes are dropped.
- **Questions** ([questions.json](questions.json)): 70 questions on first aid, water, farming and repair.
  - 36 English, worded like a search ("snake bite first aid"), including the known keyword failures.
  - 10 English in everyday words that don't use the article's terms ("my fingers went white and numb after hours in the snow").
  - 12 French and 12 Spanish, asked over the English passages. Each points to an English question; that wording stands in for the AI translating the question first (D-037).
- **Relevant passages** ([labels.py](labels.py)): a rule per question (article, words the passage must contain, and the allowed sections), reviewed by hand. On average 5 passages per question count as right.

**Recall@10** here is the share of questions with at least one right passage in the top 10. **MRR** is the average of 1/rank of the first right passage (0 if it isn't in the top 50). MRR matters because the AI only gets the first few passages.

## Candidate models

All Q8_0 GGUF files, run with llama.cpp build b11803. All Apache-2.0 or MIT.

| Model | Role | Parameters | File | License |
|---|---|---|---|---|
| multilingual-e5-small (intfloat) | embedding, 384 dims | 118 M | 132 MB | MIT |
| granite-embedding-107m-multilingual (IBM) | embedding, 384 dims | 107 M | 121 MB | Apache-2.0 |
| nomic-embed-text-v2-moe (Nomic) | embedding, 768 dims | 475 M (MoE) | 512 MB | Apache-2.0 |
| Qwen3-Embedding-0.6B (Qwen) | embedding, 1024 dims | 596 M | 639 MB | Apache-2.0 |
| bge-reranker-v2-m3 (BAAI) | reranker | 568 M | 636 MB | Apache-2.0 |
| Qwen3-Reranker-0.6B (Qwen) | reranker | 596 M | 639 MB | Apache-2.0 |
| gte-multilingual-reranker-base (Alibaba) | reranker | 306 M | 332 MB | Apache-2.0; **doesn't load** in upstream llama.cpp (architecture "new") |

Left out: bge-m3 (568 M, same size class as Qwen3-Embedding, kept out to stay under the download budget), gte-multilingual-base (same unsupported architecture as its reranker), paraphrase-multilingual-MiniLM (older, weaker), EmbeddingGemma (Gemma license, not Apache/MIT), jina-reranker-v2/v3 (**CC-BY-NC, non-commercial**, excluded).

## How to run

Needs Python 3.11+ with numpy, Docker, llama.cpp b11803 in `.local/llama-win` (Windows CUDA) and `.local/llama-linux.tar.gz` (Linux x64), and the ZIM in `.local/zim`. `.local/` is gitignored. Set `NEVO_LOCAL` if `.local` isn't at the repo root (for example in a git worktree).

```bash
bash download_models.sh                       # about 3 GB of GGUF files into .local/models/emb
docker build -t nevo-emb-py .                 # Python + libzim, for extract.py
docker run --rm -w /out -v "$NEVO_LOCAL/zim:/zim:ro" -v "$PWD:/spike:ro" -v "$PWD/data:/out" \
  nevo-emb-py python -I /spike/extract.py build /zim/wikipedia_en_medicine_nopic_2026-10.zim /spike/articles.txt 2500
python labels.py                              # review the relevant passages per question
python run_emb.py --env gpu                   # vectors and reranker scores (about 6 minutes)
python evaluate.py                            # quality table -> results/table.txt, results/metrics.json
python run_emb.py --env cpu4                  # speed and memory in Docker: 4 cores, 4 GB, no swap
```

`sanity.py MODEL` prints a few similarities, to catch a broken GGUF file (one e5-small conversion gave nearly the same score for every passage).
