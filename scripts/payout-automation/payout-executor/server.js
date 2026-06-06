const fs = require("fs");
const path = require("path");

const express = require("express");
const dotenv = require("dotenv");
const { ethers } = require("ethers");

dotenv.config();

const PORT = Number(process.env.PORT || 8787);
const EXECUTOR_API_KEY = process.env.EXECUTOR_API_KEY || "";
const BSC_RPC_URL = process.env.BSC_RPC_URL || "";
const USDC_TOKEN_ADDRESS = process.env.USDC_TOKEN_ADDRESS || "";
const TREASURY_PRIVATE_KEY = process.env.TREASURY_PRIVATE_KEY || "";
const TREASURY_MNEMONIC = process.env.TREASURY_MNEMONIC || "";
const TREASURY_DERIVATION_PATH = process.env.TREASURY_DERIVATION_PATH || "m/44'/60'/0'/0/0";
const USDC_DECIMALS = Number(process.env.USDC_DECIMALS || 6);
const ALLOW_DRY_RUN = String(process.env.ALLOW_DRY_RUN || "true").toLowerCase() === "true";

if (!EXECUTOR_API_KEY) {
  throw new Error("Missing EXECUTOR_API_KEY");
}

if (!ALLOW_DRY_RUN) {
  if (!BSC_RPC_URL || !USDC_TOKEN_ADDRESS || (!TREASURY_PRIVATE_KEY && !TREASURY_MNEMONIC)) {
    throw new Error(
      "Missing BSC executor env values. Set BSC_RPC_URL, USDC_TOKEN_ADDRESS, and either TREASURY_PRIVATE_KEY or TREASURY_MNEMONIC"
    );
  }
}

const app = express();
app.use(express.json({ limit: "256kb" }));

const statePath = path.join(__dirname, "executor-state.json");

function loadState() {
  if (!fs.existsSync(statePath)) {
    return { processed_request_ids: {}, processed_swap_refs: {} };
  }
  return JSON.parse(fs.readFileSync(statePath, "utf8"));
}

function saveState(state) {
  fs.writeFileSync(statePath, JSON.stringify(state, null, 2), "utf8");
}

function ensureAuth(req, res, next) {
  const provided = req.header("X-Api-Key") || "";
  if (!provided || provided !== EXECUTOR_API_KEY) {
    return res.status(401).json({ status: "error", error: "unauthorized" });
  }
  next();
}

function validateAddress(addr) {
  try {
    return ethers.isAddress(addr);
  } catch {
    return false;
  }
}

function getTreasuryWallet(provider) {
  if (TREASURY_PRIVATE_KEY) {
    return new ethers.Wallet(TREASURY_PRIVATE_KEY, provider);
  }

  // Fallback path for wallet setups where private-key export is unavailable.
  return ethers.HDNodeWallet.fromPhrase(
    TREASURY_MNEMONIC,
    undefined,
    TREASURY_DERIVATION_PATH
  ).connect(provider);
}

function summarizeState(state) {
  const processedRequests = Object.keys(state.processed_request_ids || {}).length;
  const processedSwaps = Object.keys(state.processed_swap_refs || {}).length;
  const recent = Object.values(state.processed_request_ids || {})
    .sort((a, b) => new Date(b.created_at || 0) - new Date(a.created_at || 0))
    .slice(0, 10);

  return {
    processed_requests: processedRequests,
    processed_swaps: processedSwaps,
    recent,
  };
}

app.get("/", (_req, res) => {
  const summary = summarizeState(loadState());
  const rows = summary.recent
    .map(
      (item) => `
        <tr>
          <td>${item.request_id || "-"}</td>
          <td>${item.usdc_amount ?? "-"}</td>
          <td>${item.mode || "-"}</td>
          <td style="max-width:360px;word-break:break-all">${item.tx_hash || "-"}</td>
          <td>${item.created_at || "-"}</td>
        </tr>`
    )
    .join("");

  const html = `<!doctype html>
  <html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width,initial-scale=1" />
    <title>ANET Payout Executor</title>
    <style>
      body { font-family: Arial, sans-serif; margin: 24px; background: #0c1222; color: #eef2ff; }
      .card { background: #121a30; border: 1px solid #223154; border-radius: 12px; padding: 16px; margin-bottom: 16px; }
      h1 { margin: 0 0 12px 0; font-size: 24px; }
      .kpi { display: inline-block; margin-right: 18px; font-size: 14px; }
      .mono { font-family: Consolas, monospace; }
      table { width: 100%; border-collapse: collapse; font-size: 13px; }
      th, td { border-bottom: 1px solid #223154; padding: 8px; text-align: left; }
      th { color: #9fb0dd; }
      a { color: #7bb2ff; }
    </style>
  </head>
  <body>
    <div class="card">
      <h1>ANET Payout Executor</h1>
      <div class="kpi">Mode: <strong>${ALLOW_DRY_RUN ? "DRY_RUN" : "LIVE"}</strong></div>
      <div class="kpi">Processed Requests: <strong>${summary.processed_requests}</strong></div>
      <div class="kpi">Processed Swaps: <strong>${summary.processed_swaps}</strong></div>
      <div class="kpi">Health: <a href="/health">/health</a></div>
      <div class="kpi">Status: <a href="/status">/status</a></div>
    </div>

    <div class="card">
      <h1 style="font-size:18px">Recent Payout Records</h1>
      <table>
        <thead>
          <tr>
            <th>Request ID</th>
            <th>USDC</th>
            <th>Mode</th>
            <th>Tx Hash</th>
            <th>Created At</th>
          </tr>
        </thead>
        <tbody>
          ${rows || '<tr><td colspan="5">No payouts yet</td></tr>'}
        </tbody>
      </table>
    </div>
  </body>
  </html>`;

  res.type("html").send(html);
});

app.get("/health", (_req, res) => {
  res.json({ status: "ok", dry_run: ALLOW_DRY_RUN });
});

app.get("/status", (_req, res) => {
  const summary = summarizeState(loadState());
  res.json({
    status: "ok",
    dry_run: ALLOW_DRY_RUN,
    signer_source: TREASURY_PRIVATE_KEY ? "private_key" : (TREASURY_MNEMONIC ? "mnemonic" : "none"),
    ...summary,
  });
});

app.post("/execute", ensureAuth, async (req, res) => {
  const {
    request_id,
    user_wallet,
    destination_bsc_address,
    usdc_amount,
    swap_reference,
    asset,
    network,
  } = req.body || {};

  if (!request_id || !destination_bsc_address || !usdc_amount) {
    return res.status(400).json({ status: "error", error: "missing required fields" });
  }

  if (!validateAddress(destination_bsc_address)) {
    return res.status(400).json({ status: "error", error: "invalid destination_bsc_address" });
  }

  if (Number(usdc_amount) <= 0) {
    return res.status(400).json({ status: "error", error: "usdc_amount must be > 0" });
  }

  if (asset && String(asset).toUpperCase() !== "USDC") {
    return res.status(400).json({ status: "error", error: "unsupported asset" });
  }

  if (network && String(network).toUpperCase() !== "BSC") {
    return res.status(400).json({ status: "error", error: "unsupported network" });
  }

  const state = loadState();
  if (state.processed_request_ids[request_id]) {
    return res.status(200).json({
      status: "ok",
      tx_hash: state.processed_request_ids[request_id].tx_hash,
      duplicate: true,
    });
  }

  if (swap_reference && state.processed_swap_refs[swap_reference]) {
    return res.status(200).json({
      status: "ok",
      tx_hash: state.processed_swap_refs[swap_reference].tx_hash,
      duplicate: true,
    });
  }

  if (ALLOW_DRY_RUN) {
    const dryHash = `dryrun-${request_id}`;
    const record = {
      request_id,
      user_wallet,
      destination_bsc_address,
      usdc_amount: Number(usdc_amount),
      tx_hash: dryHash,
      created_at: new Date().toISOString(),
      mode: "dry_run",
    };

    state.processed_request_ids[request_id] = record;
    if (swap_reference) {
      state.processed_swap_refs[swap_reference] = record;
    }
    saveState(state);

    return res.status(200).json({ status: "ok", tx_hash: dryHash, mode: "dry_run" });
  }

  try {
    const provider = new ethers.JsonRpcProvider(BSC_RPC_URL);
    const wallet = getTreasuryWallet(provider);

    const usdcAbi = [
      "function transfer(address to, uint256 value) external returns (bool)",
      "function balanceOf(address account) external view returns (uint256)"
    ];

    const usdc = new ethers.Contract(USDC_TOKEN_ADDRESS, usdcAbi, wallet);
    const amountUnits = ethers.parseUnits(String(usdc_amount), USDC_DECIMALS);

    const treasuryBal = await usdc.balanceOf(wallet.address);
    if (treasuryBal < amountUnits) {
      return res.status(400).json({ status: "error", error: "insufficient treasury USDC balance" });
    }

    const tx = await usdc.transfer(destination_bsc_address, amountUnits);
    const receipt = await tx.wait(1);

    const txHash = receipt && receipt.hash ? receipt.hash : tx.hash;

    const record = {
      request_id,
      user_wallet,
      destination_bsc_address,
      usdc_amount: Number(usdc_amount),
      tx_hash: txHash,
      created_at: new Date().toISOString(),
      mode: "live",
    };

    state.processed_request_ids[request_id] = record;
    if (swap_reference) {
      state.processed_swap_refs[swap_reference] = record;
    }
    saveState(state);

    return res.status(200).json({ status: "ok", tx_hash: txHash });
  } catch (error) {
    return res.status(500).json({ status: "error", error: String(error && error.message ? error.message : error) });
  }
});

app.listen(PORT, () => {
  console.log(`payout executor listening on :${PORT}`);
});
