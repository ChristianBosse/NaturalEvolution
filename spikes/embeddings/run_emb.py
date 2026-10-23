"""Run the embedding models and rerankers with llama-server.

    python run_emb.py --env gpu    # quality: embed every passage and query, rerank all candidates
    python run_emb.py --env cpu4   # speed and memory in Docker: 4 cores, 4 GB RAM, no swap

gpu writes vectors to results/vectors/ and reranker scores to results/rerank/ (both
gitignored, rebuilt in minutes). cpu4 writes results/cpu4/<model>.json.
Throwaway spike code: stdlib plus numpy.
"""
import argparse
import json
import os
import subprocess
import time
import urllib.request
from pathlib import Path

import numpy as np

from bm25 import BM25
from labels import load

HERE = Path(__file__).resolve().parent
LOCAL = Path(os.environ.get("NEVO_LOCAL", HERE.parents[1] / ".local"))
MODELS_DIR = LOCAL / "models" / "emb"
LLAMA_BUILD = "llama-b11803"
CONTAINER = "nevo-emb-bench"
QWEN_TASK = "Instruct: Given a question, retrieve passages that answer the question\nQuery: "

# name: (file, kind, pooling, query prefix, passage prefix, dimensions)
MODELS = {
    "e5-small": ("multilingual-e5-small-q8_0.gguf", "emb", "mean", "query: ", "passage: "),
    "granite-107m": ("granite-embedding-107m-multilingual-Q8_0.gguf", "emb", "cls", "", ""),
    "nomic-v2-moe": ("nomic-embed-text-v2-moe.Q8_0.gguf", "emb", "mean", "search_query: ", "search_document: "),
    "qwen3-emb-0.6b": ("Qwen3-Embedding-0.6B-Q8_0.gguf", "emb", "last", QWEN_TASK, ""),
    "gte-rerank": ("gte-multilingual-reranker-base-Q8_0.gguf", "rerank", None, "", ""),
    "bge-rerank-m3": ("bge-reranker-v2-m3-Q8_0.gguf", "rerank", None, "", ""),
    "qwen3-rerank-0.6b": ("qwen3-reranker-0.6b-q8_0.gguf", "rerank", None, "", ""),
}
MAX_WORDS = 250  # a few passages run to ~310 words; this keeps inputs under 512 tokens


def passage_text(p):
    return " ".join((p["head"] + ". " + p["text"]).split()[:MAX_WORDS])


SMALL = os.environ.get("NEVO_SMALL") == "1"  # node settings: one 512-token slot, smallest buffers
# --no-repack (D-033) saves RAM for the chat model, but repacked weights make batch math
# (all an embedder or reranker does) much faster on x86. NEVO_REPACK=1 measures that.
REPACK = os.environ.get("NEVO_REPACK") == "1"


def server_args(kind, pooling, threads):
    a = ["-c", "4096", "-b", "2048", "-ub", "2048", "-np", "4"] if not SMALL else ["-c", "512", "-b", "512", "-ub", "512", "-np", "1"]
    a += ["-t", str(threads), "--cache-ram", "0"] + ([] if REPACK else ["--no-repack"])
    a += ["--embedding", "--pooling", pooling] if kind == "emb" else ["--reranking"]
    return a


def start(env, name):
    file, kind, pooling, *_ = MODELS[name]
    if env == "gpu":
        port = 8193
        exe = LOCAL / "llama-win" / "llama-server.exe"
        log = open(HERE / "results" / "server.log", "w")
        proc = subprocess.Popen([str(exe), "-m", str(MODELS_DIR / file), "-ngl", "99", "--port", str(port),
                                 *server_args(kind, pooling, 8)], stdout=log, stderr=log)
        return port, lambda: (proc.terminate(), proc.wait())
    port = 8194
    subprocess.run(["docker", "rm", "-f", CONTAINER], capture_output=True)
    script = ("apt-get -qq update >/dev/null && apt-get -qq install -y libgomp1 libcurl4 >/dev/null; "
              "mkdir -p /llama && tar -xzf /nevo/llama-linux.tar.gz -C /llama && "
              f"exec /llama/{LLAMA_BUILD}/llama-server -m /nevo/models/emb/{file} --host 0.0.0.0 --port 8080 "
              + " ".join(server_args(kind, pooling, 4)))
    subprocess.run(["docker", "run", "-d", "--name", CONTAINER, "--memory", "4g", "--memory-swap", "4g", "--cpus", "4",
                    "-p", f"{port}:8080", "-v", f"{LOCAL}:/nevo:ro", "ubuntu:24.04", "bash", "-c", script],
                   check=True, capture_output=True)
    return port, lambda: subprocess.run(["docker", "rm", "-f", CONTAINER], capture_output=True)


def post(port, path, body, timeout=600):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", json.dumps(body).encode(),
                                 {"Content-Type": "application/json"})
    try:
        return json.load(urllib.request.urlopen(req, timeout=timeout))
    except urllib.error.HTTPError as e:
        raise RuntimeError(e.read()[:300]) from None


def wait_ready(port, limit=600):
    t0 = time.time()
    while time.time() - t0 < limit:
        try:
            if json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=5)).get("status") == "ok":
                return time.time() - t0
        except Exception:
            pass
        time.sleep(1)
    return None


def embed(port, texts, batch=16):
    out = []
    for i in range(0, len(texts), batch):
        try:
            r = post(port, "/v1/embeddings", {"input": texts[i:i + batch], "model": "x"})
            out += [d["embedding"] for d in sorted(r["data"], key=lambda d: d["index"])]
        except RuntimeError as e:  # a passage too long for a 512-token model: cut it until it fits
            if b"exceed_context" not in str(e).encode():
                raise
            for t in texts[i:i + batch]:
                while True:
                    try:
                        out.append(post(port, "/v1/embeddings", {"input": [t], "model": "x"})["data"][0]["embedding"])
                        break
                    except RuntimeError:
                        t = " ".join(t.split()[: int(len(t.split()) * 0.85)])
    v = np.array(out, dtype=np.float32)
    return v / np.linalg.norm(v, axis=1, keepdims=True)


def rerank(port, query, docs):
    r = post(port, "/v1/rerank", {"query": query, "documents": docs, "top_n": len(docs), "model": "x"})
    return {x["index"]: x["relevance_score"] for x in r["results"]}


def memory():
    r = subprocess.run(["docker", "exec", CONTAINER, "cat", "/sys/fs/cgroup/memory.peak"], capture_output=True, text=True)
    h = subprocess.run(["docker", "exec", CONTAINER, "bash", "-c", "grep VmHWM /proc/$(pidof llama-server)/status"],
                       capture_output=True, text=True)
    return {"cgroup_peak_mb": round(int(r.stdout) / 1e6) if r.stdout.strip() else None,
            "process_peak_rss_mb": round(int(h.stdout.split()[1]) / 1e3) if h.stdout.strip() else None}


def candidates(ps, qs, bm, vec_dir):
    """Per question: BM25 top 50 (original and English wording) plus each embedding model's top 50."""
    cand = {q["id"]: set(bm.search(q["q"])) | set(bm.search(q["en"])) for q in qs}
    for name, (_, kind, *_r) in MODELS.items():
        f = vec_dir / f"{name}.npz"
        if kind == "emb" and f.exists():
            z = np.load(f)
            sims = z["queries"] @ z["passages"].T
            for qi, q in enumerate(qs):
                cand[q["id"]] |= set(np.argsort(-sims[qi])[:50].tolist())
    return cand


def run_gpu(names, ps, qs):
    vec_dir, rr_dir = HERE / "results" / "vectors", HERE / "results" / "rerank"
    vec_dir.mkdir(parents=True, exist_ok=True)
    rr_dir.mkdir(parents=True, exist_ok=True)
    texts = [passage_text(p) for p in ps]
    bm = BM25(texts)
    for name in names:
        file, kind, pooling, qp, dp = MODELS[name]
        print(f"== {name}", flush=True)
        port, stop = start("gpu", name)
        try:
            if wait_ready(port, 120) is None:
                print("   server did not start, see results/server.log"); continue
            if kind == "emb":
                t0 = time.time()
                pv = embed(port, [dp + t for t in texts])
                qv = embed(port, [qp + q["q"] for q in qs])
                ev = embed(port, [qp + q["en"] for q in qs])
                np.savez(vec_dir / f"{name}.npz", passages=pv, queries=qv, queries_en=ev)
                print(f"   {len(texts)} passages, dim {pv.shape[1]}, {time.time() - t0:.0f} s")
            else:
                cand = candidates(ps, qs, bm, vec_dir)
                scores, t0 = {}, time.time()
                for q in qs:
                    ids = sorted(cand[q["id"]])
                    s = rerank(port, q["q"], [texts[i] for i in ids])
                    scores[q["id"]] = {ps[ids[j]]["id"]: v for j, v in s.items()}
                    if q["lang"] != "en":  # also score with the English wording (the translated question)
                        s = rerank(port, q["en"], [texts[i] for i in ids])
                        scores[q["id"] + "|en"] = {ps[ids[j]]["id"]: v for j, v in s.items()}
                json.dump(scores, open(rr_dir / f"{name}.json", "w"))
                print(f"   {sum(len(v) for v in scores.values())} pairs, {time.time() - t0:.0f} s")
        finally:
            stop()


def run_cpu4(names, ps, qs, n_passages=300, n_rerank=10):
    out_dir = HERE / "results" / "cpu4"
    out_dir.mkdir(parents=True, exist_ok=True)
    texts = [passage_text(p) for p in ps]
    sample = texts[::len(texts) // n_passages][:n_passages]
    bm = BM25(texts)
    for name in names:
        file, kind, pooling, qp, dp = MODELS[name]
        print(f"== {name}", flush=True)
        port, stop = start("cpu4", name)
        try:
            load_s = wait_ready(port)
            if load_s is None:
                print("   server did not start"); continue
            res = {"model": name, "file": file, "file_mb": round((MODELS_DIR / file).stat().st_size / 1e6),
                   "load_s": round(load_s, 1)}
            if kind == "emb":
                t0 = time.time()
                pv = embed(port, [dp + t for t in sample])
                dt = time.time() - t0
                res.update(dim=int(pv.shape[1]), passages=len(sample), passages_per_s=round(len(sample) / dt, 1))
                lat = []
                for q in qs:
                    t0 = time.time()
                    embed(port, [qp + q["q"]])
                    lat.append(time.time() - t0)
                res["query_ms_median"] = round(1000 * sorted(lat)[len(lat) // 2])
            else:
                lat, words = [], 0
                for q in qs[:: len(qs) // n_rerank][:n_rerank]:
                    docs = [texts[i] for i in bm.search(q["en"], 50)]
                    words += sum(len(d.split()) for d in docs)
                    t0 = time.time()
                    rerank(port, q["q"], docs)
                    lat.append(time.time() - t0)
                res.update(queries=len(lat), rerank50_s_median=round(sorted(lat)[len(lat) // 2], 2),
                           rerank50_s_max=round(max(lat), 2), avg_words_per_doc=round(words / (50 * len(lat))))
            res["memory"] = memory()
            print("  ", res, flush=True)
            res["small_settings"], res["repack"] = SMALL, REPACK
            suffix = ("-small" if SMALL else "") + ("-repack" if REPACK else "")
            json.dump(res, open(out_dir / f"{name}{suffix}.json", "w"), indent=1)
        finally:
            stop()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--env", choices=["gpu", "cpu4"], required=True)
    ap.add_argument("--only", nargs="*", help="model names (default: all)")
    args = ap.parse_args()
    names = args.only or list(MODELS)
    names = [n for n in names if MODELS[n][1] == "emb"] + [n for n in names if MODELS[n][1] == "rerank"]
    ps, qs = load()
    (run_gpu if args.env == "gpu" else run_cpu4)(names, ps, qs)


if __name__ == "__main__":
    main()
