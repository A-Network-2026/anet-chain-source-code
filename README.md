# ANTS Mainnet

Rust-based ANTS Mainnet node for A-Network. It performs Genesis Activation from the Web2 Ant Ledger into `config/genesis.json`, keeps ledger accounting in ANTS, displays balances in ANET, runs a fee-only Time-Based Proof-of-Work (TPoW) chain on fast transfer epochs, and can continuously settle newly mined Web2 ANTS into the running network.

## Design rules

- `1 completed session = 4,882,812 ANTS`
- `100,000,000 ANTS = 1 ANET`
- No new ANET minting exists on this ANTS Mainnet
- The Ant Ledger becomes the chain at Genesis Activation
- Web2 miners still complete 6-hour proof-of-time sessions in the backend
- Layer 1 transfer blocks can run independently at a much faster cadence
- After startup, new Web2 mining balance can be activated into the running chain as incremental ANTS settlements once the wallet reaches 1,000 completed Web2 sessions
- Validators and ANET activation require at least 1,000 completed Web2 sessions, plus no delete, ban, or flag status
- For durable deployments, keep both `genesis.json` and `chain.json` on persistent storage so redeploys do not restart the ledger from block `#0`

## Web2 sync behavior

- Genesis Activation imports the current Web2 ledger snapshot into `config/genesis.json`
- After the node starts, it can poll PostgreSQL and settle only the newly mined Web2 ANTS delta for each wallet
- Wallets below 1,000 completed Web2 sessions are not activated into Layer 1 yet and cannot send or receive ANET on Layer 1
- On upgrade, legacy under-1,000 wallets lose their remaining Web2-derived activated ANET, while transferred-in on-chain funds remain intact
- The node tracks how much Web2 balance has already been activated, so on-chain spending is not re-credited on the next sync
- Set `ANET_WEB2_SYNC_SECONDS=0` to disable periodic sync; default is `60` seconds
- `ANET_GENESIS_PATH` and `ANET_CHAIN_PATH` let you move the node state onto a persistent volume without changing CLI commands

## Commands

```bash
cargo run -- --init-genesis
cargo run -- --bootstrap
cargo run -- --start-node
cargo run -- --bootstrap --start-node
```

If Rust is not installed locally, you can still build the node through Docker or GitHub Actions.

## Environment

Use `.env.example` as the template for PostgreSQL and runtime settings.

## API

- `GET /health`
- `GET /ready`
- `GET /blocks`
- `GET /blocks/:id`
- `GET /blocks/height/:height`
- `GET /accounts/:address`
- `POST /transactions`
- `GET /web2/account/:address`
- `GET /account/full/:address`

Native DEX API (Layer 1):

- `GET /dex/pools`
- `GET /dex/pools/:symbol`
- `POST /dex/assets/mint` (ANTS Mainnet bootstrap; requires `ANET_DEX_ADMIN_KEY`)
- `POST /dex/wrap`
- `POST /dex/unwrap`
- `POST /dex/pools/create`
- `POST /dex/pools/add-liquidity`
- `POST /dex/swap/quote`
- `POST /dex/swap/execute`

ANRC-20 API (Layer 1 token foundation):

- `GET /tokens/anrc20`
- `GET /tokens/anrc20/:symbol`
- `POST /tokens/anrc20/create`
- `POST /tokens/anrc20/mint`
- `POST /tokens/anrc20/transfer`
- `POST /app/activity` (and `POST /ui/activity` alias)

Notes:

- Transfer, DEX, and ANRC-20 mutation routes use signed payload authorization.
- Seed phrase / private key transport is disabled for transaction authorization.
- Explorer auth lifecycle actions now emit on-chain `AppActivity` events (`explorer_login_success`, `explorer_logout`, and consistent `explorer_login_rejected_*` outcomes) and are finalized in blocks as audit metadata.
- Admin mutation routes also emit consistent on-chain `AppActivity` audit events for both successful usage and invalid admin-key rejections.
- Web and mobile UI telemetry can be posted to `POST /app/activity` (`source`: `web` or `inapp`, `action`: `[a-z0-9_]+`). Backend records these as on-chain `AppActivity` with `ui_` action prefix.
- Shared integration contract for UI teams: `UI_ACTIVITY_CONTRACT_2026-05-10.md`.
- Versioned JSON Schema for UI activity payload CI validation: `schemas/ui-activity/v1.schema.json`.
- Wallet must have at least `1,000` completed sessions to use DEX routes.
- DEX pools are native to this Layer 1 node and use ANET (ANTS units) against an additional network asset symbol (example: `USDA`).
- `WANET` is the recommended 1:1 wrapped ANET symbol for native DEX routing inside your own L1.
- `POST /dex/wrap` debits native ANET and credits `WANET` 1:1 inside the same eligible wallet.
- `POST /dex/unwrap` burns `WANET` and credits native ANET 1:1 back to the same eligible wallet.
- Primary market pools should still be external-value pairs such as `USDT`, `USDC`, or `WBTC`; an `ANET/WANET` pool is only a wrapper rail, not price discovery.
- Native ANET issuance remains mining-only in production. Admin mint routes require explicit `ANET_ALLOW_ADMIN_NATIVE_MINT=true` and should stay disabled in production.

Signed transaction payload example (`POST /transactions`):

```json
{
  "tx_type": "transfer",
  "from": "ANET1234567890ABCDEF1234567890ABCDEF1234",
  "to": "ANETFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
  "amount_ants": 1000,
  "fee_ants": 1000,
  "nonce": 1,
  "timestamp": "2026-05-10T08:00:00Z",
  "chain_id": "anet-mainnet",
  "payload": {
    "memo": "signed transfer"
  },
  "signature": "<65-byte-recoverable-signature-hex>",
  "tx_hash": "<sha256-canonical-hash-hex>"
}
```

Signed action authorization envelope (used by DEX and ANRC-20 mutation routes):

```json
{
  "auth": {
    "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
    "nonce": 9,
    "timestamp": "2026-05-10T08:01:00Z",
    "chain_id": "anet-mainnet",
    "payload": {
      "route": "dex_swap"
    },
    "signature": "<65-byte-recoverable-signature-hex>",
    "action_hash": "<sha256-canonical-action-hash-hex>"
  }
}
```

Wrap payload example (`POST /dex/wrap`):

```json
{
  "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
  "auth": {
    "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
    "nonce": 10,
    "timestamp": "2026-05-10T08:02:00Z",
    "chain_id": "anet-mainnet",
    "payload": {
      "route": "dex_wrap"
    },
    "signature": "<65-byte-recoverable-signature-hex>",
    "action_hash": "<sha256-canonical-action-hash-hex>"
  },
  "amount_ants": 500000000
}
```

Unwrap payload example (`POST /dex/unwrap`):

```json
{
  "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
  "auth": {
    "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
    "nonce": 11,
    "timestamp": "2026-05-10T08:03:00Z",
    "chain_id": "anet-mainnet",
    "payload": {
      "route": "dex_unwrap"
    },
    "signature": "<65-byte-recoverable-signature-hex>",
    "action_hash": "<sha256-canonical-action-hash-hex>"
  },
  "amount_units": 500000000
}
```

Create pool payload example (`POST /dex/pools/create`):

```json
{
  "provider": "ANET1234567890ABCDEF1234567890ABCDEF1234",
  "auth": {
    "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
    "nonce": 12,
    "timestamp": "2026-05-10T08:04:00Z",
    "chain_id": "anet-mainnet",
    "payload": {
      "route": "dex_create_pool"
    },
    "signature": "<65-byte-recoverable-signature-hex>",
    "action_hash": "<sha256-canonical-action-hash-hex>"
  },
  "token_symbol": "WANET",
  "anet_amount_ants": 500000000,
  "token_amount_units": 500000000,
  "fee_bps": 5
}
```

Stable pool bootstrap example (`POST /dex/pools/create`):

```json
{
  "provider": "ANET1234567890ABCDEF1234567890ABCDEF1234",
  "auth": {
    "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
    "nonce": 13,
    "timestamp": "2026-05-10T08:05:00Z",
    "chain_id": "anet-mainnet",
    "payload": {
      "route": "dex_create_pool"
    },
    "signature": "<65-byte-recoverable-signature-hex>",
    "action_hash": "<sha256-canonical-action-hash-hex>"
  },
  "token_symbol": "USDT",
  "anet_amount_ants": 500000000,
  "token_amount_units": 500000000,
  "fee_bps": 30
}
```

Swap quote payload example:

```json
{
  "token_symbol": "USDA",
  "amount_in": 10000000,
  "anet_to_token": true
}
```

## Explorer

- `/explorer` dashboard
- `/explorer/blocks`
- `/explorer/blocks/:height`
- `/explorer/accounts/:address`
- dashboard includes a Colony Transfer form that posts to `/transactions`

Example transaction payload:

```json
{
  "tx_type": "transfer",
  "from": "ANET1234567890ABCDEF1234567890ABCDEF1234",
  "to": "ANETFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
  "amount_ants": 1000,
  "fee_ants": 1000,
  "nonce": 1,
  "timestamp": "2026-05-10T08:00:00Z",
  "chain_id": "anet-mainnet",
  "payload": {
    "memo": "signed transfer"
  },
  "signature": "<65-byte-recoverable-signature-hex>",
  "tx_hash": "<sha256-canonical-hash-hex>"
}
```

This ANTS Mainnet now uses signed payload validation and nonce-based replay protection for production transaction authorization.

## Migration notes: sender_seed -> signed payloads

If your client previously submitted `sender_seed`, migrate to local-signing flow.

Required migration steps:

1. Build canonical payload locally on the client.
2. Hash locally using SHA256.
3. Sign locally using wallet private key (secp256k1 recoverable signature).
4. Submit only signed payload fields to node routes.
5. Track wallet nonce and increment monotonically per action.

Transaction route migration (`POST /transactions`):

- Old: `from`, `to`, `amount_ants`, `fee_ants`, `sender_seed`
- New: `tx_type`, `from`, `to`, `amount_ants`, `fee_ants`, `nonce`, `timestamp`, `chain_id`, `payload`, `signature`, `tx_hash`

DEX mutation route migration (`POST /dex/*`):

- Old: route params + `sender_seed`
- New: route params + `auth` object

`auth` object fields:

- `wallet`
- `nonce`
- `timestamp`
- `chain_id`
- `payload`
- `signature`
- `action_hash`

Replay and safety requirements:

- Nonce must be strictly increasing per wallet.
- Node rejects stale nonce, duplicate nonce, invalid chain_id, malformed signatures, and hash mismatch.
- For swap execution, include `min_amount_out` and `deadline_block` for slippage/deadline protection.

Production issuance policy:

- Native ANET remains mining-only in production.
- Keep `ANET_ALLOW_ADMIN_NATIVE_MINT=false` in production.

## CI Automation

GitHub Actions workflow validates UI activity payload schemas on every PR and merge to `main`.

**Workflow file:** `.github/workflows/validate-ui-activity-schema.yml`

**Validation jobs:**

1. **Schema validation**: Runs ajv-cli against all test fixtures
   - Validates that `valid/` fixtures pass schema validation
   - Ensures `invalid/` fixtures are correctly rejected
   - Verifies schema syntax and required properties

2. **Linting**: Checks schema integrity
   - Validates JSON Schema file syntax
   - Verifies `$id` versioning format
   - Confirms all required schema sections present

3. **Backend tests** (optional): Runs cargo test suite to catch handler regressions

**Test fixtures directory:** `test-fixtures/ui-activity/`

- `valid/`: Payloads that must pass validation
  - `web-login-minimal.json`: Minimal valid payload
  - `inapp-transaction-with-auth.json`: With signed authorization
  - `web-complex-with-detail.json`: With optional detail object
  - `inapp-logout.json`: Logout action

- `invalid/`: Payloads that must be rejected
  - `missing-source.json`: Required field validation
  - `missing-action.json`: Required field validation
  - `invalid-source-enum.json`: Enum boundary checking
  - `invalid-action-uppercase.json`: Action pattern enforcement
  - `invalid-action-with-special-chars.json`: Special character rejection
  - `action-too-long.json`: Length limit enforcement

**Manual validation (local development):**

```bash
# Install ajv-cli
npm install -g ajv-cli

# Validate valid fixtures
ajv validate -s schemas/ui-activity/v1.schema.json -d test-fixtures/ui-activity/valid/*.json

# Check that invalid fixtures are rejected
ajv validate -s schemas/ui-activity/v1.schema.json -d test-fixtures/ui-activity/invalid/*.json
```

**Adding new fixtures:**

When adding new UI activities to the backend:

1. Create a valid example fixture in `test-fixtures/ui-activity/valid/` with the format `{source}_{action}.json`
2. If introducing edge cases, add corresponding invalid examples in `test-fixtures/ui-activity/invalid/`
3. Commit both valid and invalid examples so CI validates the contract
4. Push PR; CI will automatically validate all fixtures

See `test-fixtures/ui-activity/README.md` for fixture documentation and `schemas/ui-activity/v1.schema.json` for the canonical schema definition.

## GitHub fork + Render quick start

If you want to run this from your fork of `https://github.com/A-Network-2026/anet-chain`, use this order:

1. Fork the repository into your own GitHub account.
2. In the fork settings, keep the default branch as `main`.
3. In Render, create a new `Web Service` from that forked repository.
4. Set the Root Directory to `A Network/anet-private-mainnet` if Render asks for a subdirectory.
5. Render can use the included `render.yaml`, or you can enter the same values manually.

Manual Render values:

```text
Environment: Rust
Build Command: cargo build --release
Start Command: ./target/release/anet-private-mainnet --bootstrap --start-node
Health Check Path: /health
```

Required environment variables:

```text
DATABASE_URL=postgres://USER:PASSWORD@HOST:5432/DBNAME
PGSSLMODE=require
RUST_LOG=info
PORT=10000
ANET_EPOCH_SECONDS=2
ANET_WEB2_SYNC_SECONDS=60
ANET_EXPLORER_DASHBOARD_CACHE_MS=5000
ANET_EXPLORER_DASHBOARD_SOFT_TIMEOUT_MS=1500
ANET_EXPLORER_DASHBOARD_BACKOFF_MS=10000
ANET_EXPLORER_ROOM_BOT_GUARD=true
ANET_DEX_ADMIN_KEY=replace-with-long-random-secret
ANET_ALLOW_ADMIN_NATIVE_MINT=false
ANET_GENESIS_PATH=/var/data/anet/config/genesis.json
ANET_CHAIN_PATH=/var/data/anet/data/chain.json
```

Notes:

- `DATABASE_URL` must point to the same PostgreSQL instance that holds your A-Network `users` table.
- `PGSSLMODE=require` is the right setting for hosted PostgreSQL on Render or most managed providers.
- `ANET_EPOCH_SECONDS=2` is the default fast transfer-block cadence. This does not change the 6-hour Web2 mining session duration.
- `ANET_EXPLORER_DASHBOARD_CACHE_MS=5000` keeps the hot dashboard metrics endpoint responsive by reusing the last in-process snapshot for 5 seconds between refreshes.
- `ANET_EXPLORER_DASHBOARD_SOFT_TIMEOUT_MS=1500` limits how long the HTML `/explorer` page will wait for production metrics before falling back to chain-only cards, preventing 8-second page stalls when PostgreSQL is slow.
- `ANET_EXPLORER_DASHBOARD_BACKOFF_MS=10000` pauses repeated dashboard metrics DB attempts for a short cooldown after timeout/error, preventing every request from paying the full soft-timeout during temporary PostgreSQL degradation.
- `ANET_EXPLORER_ROOM_BOT_GUARD=true` short-circuits known aggressive crawler scans on sequential `/explorer/rooms/referral-room-####` keys to avoid repeated DB lookup pressure.
- `ANET_ALLOW_ADMIN_NATIVE_MINT=false` keeps native ANET issuance mining-only by disabling admin native mint paths in production.
- `ANET_EXPLORER_DETAIL_CACHE_MS=10000` keeps repeated territory, colony, room, and Web2 account drilldowns fast by reusing the last in-process detail snapshot for 10 seconds.
- On Render, attach a persistent disk and point both `ANET_GENESIS_PATH` and `ANET_CHAIN_PATH` into that mounted path so block history and activated snapshots survive redeploys.
- Render will inject its own `PORT`; if so, that runtime value overrides the example above.

## Run locally

```bash
cp .env.example .env
cargo run -- --bootstrap --start-node
```

Then open:

- `http://127.0.0.1:8080/health`
- `http://127.0.0.1:8080/ready`
- `http://127.0.0.1:8080/explorer`

## Render

The included `render.yaml` builds the Rust service, starts it with `--bootstrap --start-node`, and mounts a persistent disk at `/var/data/anet`. Render will inject `PORT`, and the node binds to `0.0.0.0:$PORT` automatically when `--bind` is not set. Local development defaults to port `8080`.

## Docker

Build the container:

```bash
docker build -t anet-private-mainnet .
```

Run the node with PostgreSQL settings:

```bash
docker run --rm -p 8080:8080 --env-file .env anet-private-mainnet
```

The container starts the node with `--bootstrap --start-node` and persists chain data under `/app/data` inside the container.

## CI

GitHub Actions workflow `.github/workflows/anet-private-mainnet-ci.yml` runs on private-mainnet changes and performs:

- `cargo fmt --check`
- `cargo clippy -D warnings`
- `cargo build --release`
- upload of the Linux release binary as a workflow artifact
- `docker build`

## Releases and GHCR

GitHub Actions workflow `.github/workflows/anet-private-mainnet-release.yml` publishes deployable outputs:

- on `main`, it builds and pushes the container image to `ghcr.io/<owner>/anet-private-mainnet`
- on version tags like `v1.0.0`, it also publishes `anet-private-mainnet-linux-amd64.tar.gz` as a GitHub release asset
- every release workflow run uploads the bundled binary plus config as a workflow artifact

Pull the published container image:

```bash
docker pull ghcr.io/<owner>/anet-private-mainnet:latest
```

## Production notes

- `ANET_EPOCH_SECONDS` can override the default fast transfer-block cadence.
- Chain persistence is atomically written to `data/chain.json` and validated against `chain_id`, `genesis_time`, hash linkage, and epoch ordering on startup.
- `/health` is a lightweight liveness endpoint; `/ready` reports service readiness and PostgreSQL reachability.