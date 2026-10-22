# Spike: AI benchmark (Phase 0)

Throwaway experiment to answer the riskiest question in the roadmap: **can a small AI model running on 4 GB of RAM give useful, grounded answers?**

This is not NEVO code. It's a test harness to choose the Standard-tier model.

## What it measures

| Question | How |
|---|---|
| Are the answers good? | 20 test cases run on a GPU (answer quality doesn't depend on the hardware) |
| Does it fit in 4 GB? | Docker container capped at 4 GB RAM and 4 CPU cores; peak memory recorded |
| Is it fast enough? | Tokens per second on 4 CPU cores. A desktop CPU is faster than a Raspberry Pi, so real Pi numbers still need a Pi. |

## Test cases

[cases.json](cases.json) holds 20 cases. Each gives the model a question plus the "library" passages a NEVO search would find, with trust labels. They cover:

- Normal answers: burns, water boiling, bleach dosing, oral rehydration, bleeding, hypothermia, canning, solar wiring, rocket stove, seed saving, cold shelter, snake bites
- **"I don't know"** when the library doesn't cover the question, including an antibiotic dose (it must not invent one)
- **Partial answers:** covered part answered, uncovered part flagged
- **Conflicts:** two sources disagree; an Unknown source contradicts a Verified one on a safety-critical topic
- **Prompt injection:** a source tries to instruct the AI to give dangerous advice
- **Other languages:** questions in French and Spanish with English sources

The source passages are short test fixtures, not a medical reference.

The system prompt NEVO uses is in [prompt.md](prompt.md).

## Candidate models

All 4-bit (Q4_K_M) GGUF files with Apache-2.0 licenses.

| Model | File size | Tier tested for |
|---|---|---|
| Qwen3.5 2B | 1.2 GB | Standard (4 GB) |
| SmolLM3 3B | 1.8 GB | Standard |
| Ministral 3 3B | 2.0 GB | Standard |
| Granite 4.2 3B | 2.1 GB | Standard |
| Gemma 4 E2B | 2.9 GB | Standard (probably too big) |
| Qwen3.5 4B | 2.6 GB | Enhanced (8 GB) |
| Gemma 4 E4B | 4.7 GB | Enhanced |

## How to run

Needs Python 3.11+, llama.cpp build b11803 in `.local/llama-win` (Windows CUDA) and `.local/llama-linux.tar.gz` (Linux x64), and the model files in `.local/models`. `.local/` is gitignored.

```bash
python run_bench.py --env gpu    # quality, on the NVIDIA GPU
python run_bench.py --env cpu4   # Docker: 4 cores, 4 GB RAM cap
```

Answers, speed and memory are saved in `results/<env>/<model>.json`. Findings are in [RESULTS.md](RESULTS.md).

A larger set of 100 cases (safety traps, units, injections, more languages) is in [cases_v2.json](cases_v2.json). Run it with `--cases-file cases_v2.json`. See [CASES_V2.md](CASES_V2.md).
