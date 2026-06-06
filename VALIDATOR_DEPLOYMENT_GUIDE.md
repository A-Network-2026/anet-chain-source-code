# A Network Validator Deployment Guide

## 1. Prerequisites
- Docker Engine 24+
- Public HTTPS endpoint for RPC
- Domain name for your validator
- Firewall rule: allow inbound 8080 and outbound 443

## 2. Configure Environment
1. Copy `.env.example` to `.env`.
2. Set production values:
   - `ANET_ENV=production`
   - `ANET_CHAIN_ID=anet-mainnet`
   - `ANET_WEB2_SYNC_SECONDS=0`
   - `ANET_SEED_NODES` to at least three independent endpoints
   - `ANET_PUBLIC_RPC_ENDPOINTS` to your public endpoint plus community peers

## 3. Bootstrap Validator
Run:

```bash
./scripts/bootstrap-validator.sh
```

Health checks:

```bash
curl -fsS http://127.0.0.1:8080/health
curl -fsS http://127.0.0.1:8080/network/discovery
curl -fsS http://127.0.0.1:8080/validators/heartbeats
```

## 4. Validator Heartbeat API
Validators should send signed heartbeat payloads every 30 seconds:

- Endpoint: `POST /validators/heartbeat`
- Required fields:
  - `wallet`
  - `auth` signed action (`validator_heartbeat`)
  - optional `node_endpoint`
  - optional `client_version`

## 4.1 Mining Proof Submission (Phase 5)
Miners submit decentralized proof-of-work proofs to the chain:

- Endpoint: `POST /mining/submit-proof`
- Request format:
  ```json
  {
    "miner": "ANET1abc...",
    "nonce": 1,
    "timestamp": "2026-05-14T10:30:00Z",
    "chain_id": "anet-private-mainnet-1",
    "proof_hash": "000012ab...",
    "difficulty": 12,
    "signature": "0x1a2b3c..."
  }
  ```
- Response (200):
  ```json
  {
    "status": "ok",
    "miner": "ANET1abc...",
    "proof_hash": "000012ab...",
    "difficulty": 12
  }
  ```

### Proof Verification Steps
1. **Validate wallet format**: Must be valid ANET address
2. **Check nonce**: Must be > 0, not previously used by this miner
3. **Verify signature**: Recover signer with secp256k1, must match miner wallet
4. **Verify difficulty**: proof_hash must have >= N leading zero bits
5. **Check chain_id**: Must match node's configured chain_id
6. **Store nonce**: Persistent DB record prevents replay attacks

### Proof Inclusion in Blocks
- Accepted proofs stored in-memory buffer
- Every epoch (~2 seconds), proofs <5 minutes old included in next block
- Proofs appear in `Block.tpow_proofs` array as JSON objects
- After block creation, proofs cleared from buffer

### Example Proof Verification (Rust)
```rust
// In your validator RPC handler
async fn post_mining_proof(
    AxumState(context): AxumState<RpcContext>,
    Json(proof): Json<TPoWProofSubmission>,
) -> impl IntoResponse {
    let chain_id = context.state.read().await.chain_id.clone();

    // Verify signature and difficulty
    let miner_wallet = match verify_tpow_proof_submission(&proof, &chain_id) {
        Ok(wallet) => wallet,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiError { error: error.to_string() }),
            ).into_response();
        }
    };

    // Check nonce replay (persistent DB)
    let replay_recorded = match db::record_validator_heartbeat_nonce(
        chain_db.as_ref(),
        &chain_id,
        &miner_wallet,
        proof.nonce,
    ).await {
        Ok(recorded) => recorded,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "Failed to verify proof nonce".into() }),
            ).into_response();
        }
    };

    if !replay_recorded {
        return (
            StatusCode::CONFLICT,
            Json(ApiError { error: "Replayed nonce".into() }),
        ).into_response();
    }

    // Record proof in state for block inclusion
    let mut state = context.state.write().await;
    state.record_mining_proof(
        miner_wallet.clone(),
        proof.proof_hash.clone(),
        proof.difficulty,
    );

    Json(MiningProofResponse {
        status: "ok",
        miner: miner_wallet,
        proof_hash: proof.proof_hash,
        difficulty: proof.difficulty,
    }).into_response()
}
```

## 5. Outage Recovery
If node data is corrupted or host is replaced:

```bash
./scripts/recover-node.sh /path/to/snapshot.tar.gz
```

Then validate:

```bash
curl -fsS http://127.0.0.1:8080/ready
curl -fsS http://127.0.0.1:8080/network/discovery
```

## 6. Rollback
To rollback application code:
1. Checkout previous stable git commit.
2. Rebuild and restart compose stack.
3. Verify `/health`, `/ready`, and `/network/discovery`.

## 7. Security Baseline
- Terminate TLS at edge proxy and only expose HTTPS publicly.
- Keep admin flags disabled in production.
- Restrict SSH by IP allowlist.
- Rotate host keys and secrets every 30 days.
- Monitor repeated 401/429 spikes for abuse patterns.
