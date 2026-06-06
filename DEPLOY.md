# Production Deploy Checklist – anet-chain (Rust L1)
## DO NOT SKIP ANY STEP

---

## Pre-Deploy (5 min)
- [ ] No uncommitted changes: `git status --short`
- [ ] Note current commit hash for rollback: `git rev-parse HEAD`
- [ ] Confirm `Cargo.toml` version bump is intentional
- [ ] Run `cargo check` locally — zero errors
- [ ] Confirm `subtle = "2"` present in Cargo.toml (constant-time key comparison)
- [ ] Confirm CORS origin is locked to `https://a-network.net` in `src/rpc.rs`
- [ ] Confirm all admin key comparisons use `constant_time_key_eq()` in `src/rpc.rs`

## Deploy
- [ ] Push to main (triggers Render auto-deploy)
- [ ] Open Render dashboard → confirm new deploy triggered
- [ ] Wait for build to complete (Rust compile ~3-5 min)
- [ ] Check deploy logs: zero panic / ICE errors

## Post-Deploy Health (5 min)
- [ ] `curl https://anet-private-mainnet.onrender.com/stats/network` → HTTP 200
- [ ] `curl https://anet-private-mainnet.onrender.com/dex/pools` → HTTP 200, valid JSON
- [ ] Check Render logs: zero ERROR lines in first 60 seconds

## Rollback
```bash
git revert <HASH>
git push origin main
# Or: Render dashboard → Manual Deploy → select previous deploy
```

## Approval
Date: ______ | Approver: ______ | Commit: ______
