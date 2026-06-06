#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ ! -f .env ]]; then
  echo "Missing .env file. Copy .env.example to .env and configure production values."
  exit 1
fi

if [[ ! -f config/genesis.json ]]; then
  echo "Missing config/genesis.json"
  exit 1
fi

echo "[1/4] Building validator image"
docker compose -f scripts/docker-compose.decentralized.yml build

echo "[2/4] Starting validator"
docker compose -f scripts/docker-compose.decentralized.yml up -d

echo "[3/4] Waiting for health"
for i in {1..30}; do
  if curl -fsS "http://127.0.0.1:8080/health" >/dev/null 2>&1; then
    break
  fi
  sleep 2
  if [[ "$i" -eq 30 ]]; then
    echo "Validator health check failed"
    exit 1
  fi
done

echo "[4/4] Validator online"
curl -fsS "http://127.0.0.1:8080/network/discovery" | sed -e 's/{/\n{/g'

echo "Done. Announce your public RPC endpoint and keep seed list updated."
