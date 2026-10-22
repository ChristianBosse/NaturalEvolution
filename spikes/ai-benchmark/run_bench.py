"""NEVO AI benchmark spike.

Runs each candidate model through the test cases with llama.cpp's server and
saves answers, speed and memory use. Throwaway spike code: stdlib only.

  python run_bench.py --env gpu            # quality run on the NVIDIA GPU
  python run_bench.py --env cpu4           # Docker: 4 CPU cores, 4 GB RAM cap
  python run_bench.py --env gpu --models qwen3.5-2b --cases burn-basic
  python run_bench.py --env gpu --models gemma4-e4b --cases-file cases_v2.json
"""

import argparse
import json
import re
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
LOCAL = REPO / ".local"  # replaced by --local (e.g. when running from a git worktree)
LLAMA_BUILD = "llama-b11803"

MODELS = {
    "qwen3.5-2b": ("Qwen3.5-2B-Q4_K_M.gguf", "standard"),
    "smollm3-3b": ("SmolLM3-Q4_K_M.gguf", "standard"),
    "ministral3-3b": ("Ministral-3-3B-Instruct-2512-Q4_K_M.gguf", "standard"),
    "granite4.2-3b": ("granite-4.2-3b-Q4_K_M.gguf", "standard"),
    "gemma4-e2b": ("gemma-4-E2B-it-Q4_K_M.gguf", "standard"),
    "qwen3.5-4b": ("Qwen3.5-4B-Q4_K_M.gguf", "enhanced"),
    "gemma4-e4b": ("gemma-4-E4B-it-Q4_K_M.gguf", "enhanced"),
}

CTX = 4096
MAX_TOKENS = 700
# One request at a time and no host-RAM prompt cache, like a 4 GB NEVO node.
# (llama-server's default prompt cache can grow to 8 GB and skews memory numbers.)
# --no-repack: on x86 the repacked copy of the weights sits next to the mmapped
# file, doubling RAM for no measurable speed gain (measured with Ministral 3B).
NODE_FLAGS = ["-np", "1", "--cache-ram", "0", "--no-repack"]
CONTAINER = "nevo-bench"


def start_server(env, model_file):
    """Start llama-server and return (port, handle to stop it)."""
    if env == "gpu":
        port = 8091
        exe = LOCAL / "llama-win" / "llama-server.exe"
        proc = subprocess.Popen(
            [str(exe), "-m", str(LOCAL / "models" / model_file), "-ngl", "99",
             "-t", "8", "-c", str(CTX), "--port", str(port), "--reasoning", "off", *NODE_FLAGS],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return port, lambda: (proc.terminate(), proc.wait())

    port = 8092
    subprocess.run(["docker", "rm", "-f", CONTAINER], capture_output=True)
    script = (
        "apt-get -qq update >/dev/null && apt-get -qq install -y libgomp1 libcurl4 >/dev/null; "
        "mkdir -p /llama && tar -xzf /nevo/llama-linux.tar.gz -C /llama && "
        f"exec /llama/{LLAMA_BUILD}/llama-server -m /nevo/models/{model_file} "
        f"-t 4 -c {CTX} --host 0.0.0.0 --port 8080 --reasoning off {' '.join(NODE_FLAGS)}")
    subprocess.run(
        ["docker", "run", "-d", "--name", CONTAINER, "--memory", "4g", "--memory-swap", "4g",
         "--cpus", "4", "-p", f"{port}:8080", "-v", f"{LOCAL}:/nevo:ro",
         "ubuntu:24.04", "bash", "-c", script],
        check=True, capture_output=True)
    return port, lambda: subprocess.run(["docker", "rm", "-f", CONTAINER], capture_output=True)


def wait_ready(port, timeout=900):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=5) as r:
                if r.status == 200:
                    return True
        except (urllib.error.URLError, ConnectionError, OSError):
            pass
        time.sleep(2)
    return False


def build_messages(system_prompt, case):
    lines = [f"Topic: {case['topic']}" + (" (safety-critical)" if case["safety_critical"] else ""),
             "", "Sources:"]
    for s in case["sources"]:
        lines += [f"[{s['id']}] ({s['trust']}) {s['title']}", s["text"], ""]
    lines += ["Question:", case["question"]]
    return [{"role": "system", "content": system_prompt},
            {"role": "user", "content": "\n".join(lines)}]


def ask(port, messages):
    body = json.dumps({
        "messages": messages, "temperature": 0, "seed": 42, "max_tokens": MAX_TOKENS,
        "chat_template_kwargs": {"enable_thinking": False},
    }).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", data=body,
                                 headers={"Content-Type": "application/json"})
    start = time.time()
    with urllib.request.urlopen(req, timeout=1800) as r:
        data = json.load(r)
    msg = data["choices"][0]["message"]
    return {
        "answer": msg.get("content") or "",
        "reasoning_chars": len(msg.get("reasoning_content") or ""),
        "finish_reason": data["choices"][0].get("finish_reason"),
        "timings": data.get("timings", {}),
        "wall_seconds": round(time.time() - start, 2),
    }


def auto_checks(case, answer):
    text = answer.lower()
    expect = case["expect"]
    missing = [group for group in expect.get("must_include_any", [])
               if not any(alt.lower() in text for alt in group)]
    forbidden = [p for p in expect.get("must_not_match", []) if re.search(p, text)]
    valid_ids = {s["id"] for s in case["sources"]}
    cited = set(re.findall(r"\[(S\d+)\]", answer))
    return {
        "missing": missing,
        "forbidden": forbidden,
        "cites_sources": bool(cited) or case["kind"] == "idk",
        "bad_citations": sorted(cited - valid_ids),
        "pass": not missing and not forbidden and not (cited - valid_ids),
    }


def container_memory_peak_mb():
    r = subprocess.run(["docker", "exec", CONTAINER, "cat", "/sys/fs/cgroup/memory.peak"],
                       capture_output=True, text=True)
    hwm = subprocess.run(["docker", "exec", CONTAINER, "bash", "-c",
                          "grep VmHWM /proc/$(pgrep -f llama-server | head -1)/status"],
                         capture_output=True, text=True)
    out = {}
    if r.returncode == 0 and r.stdout.strip().isdigit():
        out["cgroup_peak_mb"] = round(int(r.stdout) / 1e6)
    m = re.search(r"(\d+)\s*kB", hwm.stdout)
    if m:
        out["process_peak_rss_mb"] = round(int(m.group(1)) / 1e3)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--env", choices=["gpu", "cpu4"], required=True)
    ap.add_argument("--models", default="all")
    ap.add_argument("--cases", default="all", help="comma-separated case ids, or all")
    ap.add_argument("--cases-file", default="cases.json",
                    help="case file next to this script; cases_v2.json writes to results/<env>_v2/")
    ap.add_argument("--local", help="folder with llama.cpp and models (default: <repo>/.local)")
    args = ap.parse_args()
    if args.local:
        global LOCAL
        LOCAL = Path(args.local).resolve()

    system_prompt = (HERE / "prompt.md").read_text(encoding="utf-8")
    cases_path = HERE / args.cases_file
    cases = json.loads(cases_path.read_text(encoding="utf-8"))["cases"]
    if args.cases != "all":
        wanted = set(args.cases.split(","))
        cases = [c for c in cases if c["id"] in wanted]
    names = list(MODELS) if args.models == "all" else args.models.split(",")

    # cases.json -> results/<env>; cases_v2.json -> results/<env>_v2 (never overwrite v1 results)
    suffix = cases_path.stem.removeprefix("cases")
    out_dir = HERE / "results" / (args.env + suffix)
    out_dir.mkdir(parents=True, exist_ok=True)

    for name in names:
        model_file, tier = MODELS[name]
        if not (LOCAL / "models" / model_file).exists():
            print(f"skip {name}: model file missing")
            continue
        print(f"== {name} ({args.env})", flush=True)
        t0 = time.time()
        port, stop = start_server(args.env, model_file)
        try:
            if not wait_ready(port):
                print(f"   server did not start for {name}")
                result = {"model": name, "env": args.env, "error": "server did not start"}
                if args.env == "cpu4":
                    logs = subprocess.run(["docker", "logs", "--tail", "30", CONTAINER],
                                          capture_output=True, text=True)
                    result["log_tail"] = (logs.stdout + logs.stderr)[-3000:]
                (out_dir / f"{name}.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
                continue
            load_seconds = round(time.time() - t0, 1)
            rows = []
            for case in cases:
                try:
                    r = ask(port, build_messages(system_prompt, case))
                except Exception as e:  # keep going: one failure shouldn't stop the run
                    r = {"answer": "", "error": repr(e), "timings": {}}
                r["id"] = case["id"]
                r["checks"] = auto_checks(case, r["answer"])
                rows.append(r)
                t = r.get("timings", {})
                print(f"   {case['id']:<22} pass={r['checks']['pass']!s:<5} "
                      f"gen={t.get('predicted_per_second', 0):6.1f} tok/s  "
                      f"prompt={t.get('prompt_per_second', 0):7.1f} tok/s", flush=True)
            memory = container_memory_peak_mb() if args.env == "cpu4" else {}
        finally:
            stop()

        gen = [r["timings"].get("predicted_per_second", 0) for r in rows if r.get("timings")]
        summary = {
            "model": name, "file": model_file, "tier": tier, "env": args.env,
            "load_seconds": load_seconds,
            "auto_pass": sum(r["checks"]["pass"] for r in rows), "cases": len(rows),
            "avg_gen_tok_s": round(sum(gen) / len(gen), 1) if gen else None,
            "memory": memory,
        }
        (out_dir / f"{name}.json").write_text(
            json.dumps({"summary": summary, "rows": rows}, indent=2, ensure_ascii=False),
            encoding="utf-8")
        print(f"   summary: {summary}", flush=True)


if __name__ == "__main__":
    main()
