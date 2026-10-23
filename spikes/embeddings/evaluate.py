"""Score every search pipeline on the test questions.

    python evaluate.py          # table to stdout, details to results/metrics.json

Recall@10 here means: share of questions with at least one relevant passage in
the top 10. MRR is the mean of 1/rank of the first relevant passage (0 if not in
the top 50).
"""
import json
from pathlib import Path

import numpy as np

from bm25 import BM25
from labels import load
from run_emb import MODELS, passage_text

HERE = Path(__file__).resolve().parent
EMB = [n for n, m in MODELS.items() if m[1] == "emb"]
RR = [n for n, m in MODELS.items() if m[1] == "rerank"]
DEPTH = 50
GROUPS = (("all", lambda q: True),
          ("en", lambda q: q["lang"] == "en" and "style" not in q),  # English, keyword-like wording
          ("lay", lambda q: q.get("style") == "lay"),  # English, everyday words, no article terms
          ("fr+es", lambda q: q["lang"] != "en"))  # French and Spanish over English passages


def metrics(ranked, qs):
    """ranked: {qid: [passage index, ...]} -> per-group recall@10 and MRR."""
    out = {}
    for group, sel in GROUPS:
        hits, rr = [], []
        for q in filter(sel, qs):
            rel = set(q["rel_idx"])
            rank = next((r for r, i in enumerate(ranked[q["id"]][:DEPTH], 1) if i in rel), None)
            hits.append(bool(rank and rank <= 10))
            rr.append(1 / rank if rank else 0)
        out[group] = {"recall10": round(sum(hits) / len(hits), 3), "mrr": round(sum(rr) / len(rr), 3)}
    return out


def rrf(*lists, k=60):
    s = {}
    for lst in lists:
        for r, i in enumerate(lst):
            s[i] = s.get(i, 0) + 1 / (k + r + 1)
    return sorted(s, key=lambda i: -s[i])


def main():
    ps, qs = load()
    index = {p["id"]: i for i, p in enumerate(ps)}
    for q in qs:
        q["rel_idx"] = [index[r] for r in q["relevant"]]
    bm = BM25([passage_text(p) for p in ps])
    runs = {}
    bm_orig = {q["id"]: bm.search(q["q"], DEPTH) for q in qs}
    bm_en = {q["id"]: bm.search(q["en"], DEPTH) for q in qs}
    runs["BM25"] = bm_orig
    runs["BM25, question translated"] = bm_en
    emb = {}
    for n in EMB:
        f = HERE / "results" / "vectors" / f"{n}.npz"
        if not f.exists():
            continue
        z = np.load(f)
        emb[n] = {q["id"]: np.argsort(-(z["passages"] @ z["queries"][i]))[:DEPTH].tolist() for i, q in enumerate(qs)}
        runs[f"emb {n}"] = emb[n]
        runs[f"RRF BM25 translated + {n}"] = {q: rrf(bm_en[q], emb[n][q])[:DEPTH] for q in emb[n]}
        # smaller index: int8 vectors (one scale per vector), and Qwen3's Matryoshka cut to 384 dims
        def int8(v):
            s = np.abs(v).max(1, keepdims=True) / 127
            return np.round(v / s) * s

        variants = {"int8": int8}
        if n.startswith("qwen3"):
            variants["384 dims"] = lambda v: v[:, :384] / np.linalg.norm(v[:, :384], axis=1, keepdims=True)
        for label, f_ in variants.items():
            pv, qv = f_(z["passages"]), f_(z["queries"])
            runs[f"emb {n}, {label}"] = {q["id"]: np.argsort(-(pv @ qv[i]))[:DEPTH].tolist() for i, q in enumerate(qs)}
    for r in RR:
        f = HERE / "results" / "rerank" / f"{r}.json"
        if not f.exists():
            continue
        sc = {q: {index[p]: v for p, v in d.items()} for q, d in json.load(open(f)).items()}

        def rerank(cands, en=False):
            return {q: sorted(c[:DEPTH], key=lambda i: -sc.get(q + "|en" if en else q, sc[q]).get(i, -1e9))
                    for q, c in cands.items()}

        runs[f"BM25 top50 > {r}"] = rerank(bm_orig)
        runs[f"BM25 translated top50 > {r}"] = rerank(bm_en)
        runs[f"BM25 translated top50 > {r}, English query"] = rerank(bm_en, en=True)
        for n in emb:
            runs[f"emb {n} top50 > {r}"] = rerank(emb[n])
    res = {name: metrics(ranked, qs) for name, ranked in runs.items()}
    # where the snake-bite passage lands, the case D-037 calls out
    snake = {name: next((r for r, i in enumerate(ranked["snake-fa"], 1) if i in set(qs[0]["rel_idx"])), None)
             for name, ranked in runs.items()}
    json.dump({"metrics": res, "snake_fa_rank": snake,
               "per_question_rank": {name: {q["id"]: next((r for r, i in enumerate(ranked[q["id"]], 1) if i in set(q["rel_idx"])), None)
                                            for q in qs} for name, ranked in runs.items()}},
              open(HERE / "results" / "metrics.json", "w"), indent=1)
    print(f'{"pipeline (R@10 / MRR)":44}' + "".join(f"{g:>12}" for g, _ in GROUPS) + "  snake")
    for name, m in res.items():
        print(f"{name[:44]:44}" + "".join(f'  {m[g]["recall10"]:4.2f}/{m[g]["mrr"]:.2f}' for g, _ in GROUPS) + f"  {snake[name]}")


if __name__ == "__main__":
    main()
