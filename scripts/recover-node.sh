#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

SNAPSHOT_PATH="${1:-}"
if [[ -z "$SNAPSHOT_PATH" ]]; then
  echo "Usage: ./scripts/recover-node.sh /path/to/snapshot.tar.gz"
  exit 1
fi

if [[ ! -f "$SNAPSHOT_PATH" ]]; then
  echo "Snapshot file not found: $SNAPSHOT_PATH"
  exit 1
fi

echo "Stopping validator"
docker compose -f scripts/docker-compose.decentralized.yml down

echo "Restoring snapshot"
rm -rf data
mkdir -p data
tar -xzf "$SNAPSHOT_PATH" -C data

echo "Starting validator from restored state"
docker compose -f scripts/docker-compose.decentralized.yml up -d

echo "Recovery started. Verify sync:"
echo "  curl -fsS http://127.0.0.1:8080/ready"
echo "  curl -fsS http://127.0.0.1:8080/network/discovery"
