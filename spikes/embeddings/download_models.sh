#!/usr/bin/env bash
# Download the GGUF files tested in this spike into .local/models/emb (gitignored).
# All are Apache-2.0 or MIT. About 3.0 GB in total.
set -euo pipefail
# NEVO_LOCAL defaults to the repo's .local folder.
DEST="${NEVO_LOCAL:-$(cd "$(dirname "$0")/../.." && pwd)/.local}/models/emb"
mkdir -p "$DEST"
HF=https://huggingface.co
while read -r repo file; do
  [ -f "$DEST/$file" ] && { echo "have $file"; continue; }
  echo "get  $file"
  curl -sSfL -o "$DEST/$file.part" "$HF/$repo/resolve/main/$file" && mv "$DEST/$file.part" "$DEST/$file"
done <<'EOF'
TwinSunsLLC/multilingual-e5-small-gguf multilingual-e5-small-q8_0.gguf
bartowski/granite-embedding-107m-multilingual-GGUF granite-embedding-107m-multilingual-Q8_0.gguf
nomic-ai/nomic-embed-text-v2-moe-GGUF nomic-embed-text-v2-moe.Q8_0.gguf
Qwen/Qwen3-Embedding-0.6B-GGUF Qwen3-Embedding-0.6B-Q8_0.gguf
gpustack/gte-multilingual-reranker-base-GGUF gte-multilingual-reranker-base-Q8_0.gguf
gpustack/bge-reranker-v2-m3-GGUF bge-reranker-v2-m3-Q8_0.gguf
ggml-org/Qwen3-Reranker-0.6B-Q8_0-GGUF qwen3-reranker-0.6b-q8_0.gguf
EOF
ls -l "$DEST"
