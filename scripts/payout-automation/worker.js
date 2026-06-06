const fs = require("fs");
const path = require("path");

const CONFIG_PATH = process.env.CONFIG_PATH || "./config.json";
const QUEUE_PATH = process.env.QUEUE_PATH || "./queue.json";
const STATE_PATH = process.env.STATE_PATH || "./state.json";
const BASE_URL = process.env.BASE_URL || "https://anet-private-mainnet.onrender.com";
const PAYOUT_EXECUTOR_URL =
  process.env.PAYOUT_EXECUTOR_URL || "https://anet-payout-executor.onrender.com/execute";
const PAYOUT_EXECUTOR_API_KEY = process.env.PAYOUT_EXECUTOR_API_KEY || "";
const OPERATIONS_REPORT_PATH =
  process.env.OPERATIONS_REPORT_PATH || path.join(path.dirname(STATE_PATH), "operations-report.json");

function loadJson(filePath) {
  const raw = fs.readFileSync(filePath, "utf8");
  const normalized = raw.replace(/^\uFEFF/, "").trim();
  return JSON.parse(normalized);
}

function ensureParentDirectory(filePath) {
  const parentDir = path.dirname(filePath);
  if (!fs.existsSync(parentDir)) {
    fs.mkdirSync(parentDir, { recursive: true });
  }
}

function saveJson(filePath, obj) {
  ensureParentDirectory(filePath);
  fs.writeFileSync(filePath, JSON.stringify(obj, null, 2), "utf8");
}

function ensureConfigExists() {
  if (fs.existsSync(CONFIG_PATH)) {
    return;
  }

  const defaultConfig = {
    base_url: BASE_URL,
    payout_executor_url: PAYOUT_EXECUTOR_URL,
    payout_executor_api_key: PAYOUT_EXECUTOR_API_KEY,
    dry_run: false,
    min_sessions: 1000,
    max_payout_usdc_per_user_per_day: 5,
    max_total_payout_usdc_per_hour: 25,
    reserve_ratio_min: 1.2,
    current_reserve_usdc: 100,
    current_pending_liabilities_usdc: 0,
    max_retries: 3,
    chain_activity_enabled: true,
    chain_activity_source: "inapp",
  };

  saveJson(CONFIG_PATH, defaultConfig);
  console.log(`Created default config at ${CONFIG_PATH}`);
}

function ensureQueueExists() {
  if (!fs.existsSync(QUEUE_PATH)) {
    saveJson(QUEUE_PATH, { items: [] });
    console.log(`Created default queue at ${QUEUE_PATH}`);
  }
}

function ensureStateExists() {
  if (fs.existsSync(STATE_PATH)) {
    return;
  }

  const defaultState = {
    processed_request_ids: [],
    processed_swap_references: [],
    hourly_window_start: new Date().toISOString(),
    hourly_paid_total_usdc: 0,
    user_daily_paid_usdc: {},
  };
  saveJson(STATE_PATH, defaultState);
  console.log(`Created default state at ${STATE_PATH}`);
}

function ensureStateShape(state) {
  state.processed_request_ids = Array.isArray(state.processed_request_ids)
    ? state.processed_request_ids
    : [];
  state.processed_swap_references = Array.isArray(state.processed_swap_references)
    ? state.processed_swap_references
    : [];
  state.user_daily_paid_usdc = state.user_daily_paid_usdc && typeof state.user_daily_paid_usdc === "object"
    ? state.user_daily_paid_usdc
    : {};
  state.hourly_paid_total_usdc = Number(state.hourly_paid_total_usdc || 0);
  state.hourly_window_start = state.hourly_window_start || new Date().toISOString();
}

function getReserveRatio(config) {
  const liabilities = Number(config.current_pending_liabilities_usdc || 0);
  const reserve = Number(config.current_reserve_usdc || 0);
  if (liabilities <= 0) {
    return 999999.0;
  }
  return reserve / liabilities;
}

function ensureHourlyWindow(state) {
  const now = new Date();
  const start = new Date(state.hourly_window_start);
  if (Number.isNaN(start.getTime()) || (now.getTime() - start.getTime()) / 3600000 >= 1) {
    state.hourly_window_start = now.toISOString();
    state.hourly_paid_total_usdc = 0;
  }
}

function getUserDayKey(wallet) {
  const today = new Date().toISOString().slice(0, 10);
  return `${today}|${String(wallet || "").toUpperCase()}`;
}

function buildTodayCapUsage(state) {
  const today = new Date().toISOString().slice(0, 10);
  const rows = [];
  const map = state && state.user_daily_paid_usdc && typeof state.user_daily_paid_usdc === "object"
    ? state.user_daily_paid_usdc
    : {};

  for (const [key, value] of Object.entries(map)) {
    if (!key.startsWith(`${today}|`)) {
      continue;
    }
    const wallet = key.slice(today.length + 1);
    const paid = Number(value || 0);
    if (!wallet || !Number.isFinite(paid)) {
      continue;
    }
    rows.push({ wallet, paid_usdc: paid });
  }

  rows.sort((a, b) => b.paid_usdc - a.paid_usdc);
  return rows.slice(0, 10);
}

function loadOperationsReport(filePath) {
  if (!fs.existsSync(filePath)) {
    return { schema: 1, history: [] };
  }
  try {
    const data = loadJson(filePath);
    if (!data || typeof data !== "object") {
      return { schema: 1, history: [] };
    }
    if (!Array.isArray(data.history)) {
      data.history = [];
    }
    data.schema = Number(data.schema || 1);
    return data;
  } catch {
    return { schema: 1, history: [] };
  }
}

function saveOperationsCycle(reportPath, cycle) {
  const report = loadOperationsReport(reportPath);
  report.latest = cycle;
  report.history.push(cycle);
  if (report.history.length > 500) {
    report.history = report.history.slice(report.history.length - 500);
  }
  saveJson(reportPath, report);
}

function normalizeCandidateRecords(payload) {
  if (Array.isArray(payload)) {
    return payload;
  }
  if (!payload || typeof payload !== "object") {
    return [];
  }
  if (Array.isArray(payload.items)) {
    return payload.items;
  }
  if (Array.isArray(payload.candidates)) {
    return payload.candidates;
  }
  if (Array.isArray(payload.users)) {
    return payload.users;
  }
  return [];
}

function buildExistingRequestIds(queue, state) {
  const ids = new Set(Array.isArray(state.processed_request_ids) ? state.processed_request_ids : []);
  for (const item of Array.isArray(queue.items) ? queue.items : []) {
    if (item && item.request_id) {
      ids.add(String(item.request_id));
    }
  }
  return ids;
}

function buildExistingSwapRefs(queue, state) {
  const refs = new Set(Array.isArray(state.processed_swap_references) ? state.processed_swap_references : []);
  for (const item of Array.isArray(queue.items) ? queue.items : []) {
    if (item && item.swap_reference) {
      refs.add(String(item.swap_reference));
    }
  }
  return refs;
}

function isLikelyEvmAddress(value) {
  return typeof value === "string" && /^0x[a-fA-F0-9]{40}$/.test(value);
}

async function autoIngestQueue(config, queue, state) {
  const enabled = config.auto_ingest_enabled !== undefined ? Boolean(config.auto_ingest_enabled) : false;
  if (!enabled) {
    return { added: 0, skipped: 0, reason: "disabled" };
  }

  const sourceUrl = String(config.auto_ingest_url || "").trim();
  if (!sourceUrl) {
    return { added: 0, skipped: 0, reason: "missing auto_ingest_url" };
  }

  const maxItems = Number(config.auto_ingest_limit || 50);
  const defaultAmount = Number(config.auto_ingest_default_usdc_amount || 1);
  const nowIso = new Date().toISOString();
  const existingRequestIds = buildExistingRequestIds(queue, state);
  const existingSwapRefs = buildExistingSwapRefs(queue, state);
  let added = 0;
  let skipped = 0;

  const headers = {};
  if (config.auto_ingest_api_key) {
    headers["X-Api-Key"] = String(config.auto_ingest_api_key);
  }

  const payload = await fetchJson(sourceUrl, { method: "GET", headers });
  const records = normalizeCandidateRecords(payload).slice(0, Math.max(0, maxItems));

  for (const raw of records) {
    const userWallet = String(raw.user_wallet || raw.wallet || "").trim().toUpperCase();
    const destination = String(raw.destination_bsc_address || raw.destination || "").trim();
    const amount = Number(raw.usdc_amount ?? defaultAmount);

    if (!userWallet || !destination || !Number.isFinite(amount) || amount <= 0 || !isLikelyEvmAddress(destination)) {
      skipped += 1;
      continue;
    }

    const requestId = String(raw.request_id || `auto_${userWallet}_${destination.toLowerCase()}_${nowIso}`);
    const swapRef = String(raw.swap_reference || `auto_swap_${userWallet}_${destination.toLowerCase()}`);

    if (existingRequestIds.has(requestId) || existingSwapRefs.has(swapRef)) {
      skipped += 1;
      continue;
    }

    queue.items.push({
      request_id: requestId,
      user_wallet: userWallet,
      destination_bsc_address: destination,
      usdc_amount: amount,
      swap_reference: swapRef,
      status: "pending",
      retries: 0,
      created_at: nowIso,
      last_error: "",
      updated_at: nowIso,
    });

    existingRequestIds.add(requestId);
    existingSwapRefs.add(swapRef);
    added += 1;
  }

  return { added, skipped, reason: "ok" };
}

async function fetchJson(url, options = {}) {
  const response = await fetch(url, options);
  const text = await response.text();
  let data = {};
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = { raw: text };
    }
  }
  if (!response.ok) {
    const message = typeof data.error === "string" ? data.error : `${response.status} ${response.statusText}`;
    throw new Error(message);
  }
  return data;
}

async function getWeb2Eligibility(baseUrl, wallet) {
  try {
    const normalizedBase = String(baseUrl || "").replace(/\/+$/, "");
    const response = await fetchJson(`${normalizedBase}/web2/account/${wallet}`);
    return {
      found: true,
      sessions: Number(response.sessions || 0),
      is_eligible: Boolean(response.is_eligible),
    };
  } catch {
    return {
      found: false,
      sessions: 0,
      is_eligible: false,
    };
  }
}

async function invokePayoutExecutor(config, item) {
  if (!config.payout_executor_url) {
    return { ok: false, error: "payout_executor_url is not configured", tx_hash: "" };
  }

  const headers = { "Content-Type": "application/json" };
  if (config.payout_executor_api_key) {
    headers["X-Api-Key"] = config.payout_executor_api_key;
  }

  const payload = {
    request_id: item.request_id,
    user_wallet: item.user_wallet,
    destination_bsc_address: item.destination_bsc_address,
    usdc_amount: Number(item.usdc_amount),
    asset: "USDC",
    network: "BSC",
    swap_reference: item.swap_reference,
  };

  try {
    const response = await fetchJson(config.payout_executor_url, {
      method: "POST",
      headers,
      body: JSON.stringify(payload),
    });

    const ok = response.status === "ok" || response.status === "sent" || response.ok === true;
    return {
      ok,
      error: ok ? "" : "executor returned non-ok response",
      tx_hash: String(response.tx_hash || ""),
    };
  } catch (error) {
    return {
      ok: false,
      error: error.message,
      tx_hash: "",
    };
  }
}

async function invokeOnchainPayoutActivity(config, item, txHash) {
  const enabled = config.chain_activity_enabled !== undefined ? Boolean(config.chain_activity_enabled) : true;
  if (!enabled) {
    return { ok: true, error: "" };
  }

  let activityUrl = String(config.chain_activity_url || "").trim();
  if (!activityUrl) {
    const baseUrl = String(config.base_url || "").replace(/\/+$/, "");
    if (!baseUrl) {
      return { ok: false, error: "base_url is empty for chain activity" };
    }
    activityUrl = `${baseUrl}/app/activity`;
  }

  const source = config.chain_activity_source === "web" ? "web" : "inapp";
  const payload = {
    source,
    action: "payout_sent",
    status: "success",
    detail: `request_id=${item.request_id};wallet=${item.user_wallet};destination=${item.destination_bsc_address};usdc=${item.usdc_amount};tx_hash=${txHash}`,
  };

  try {
    const response = await fetchJson(activityUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    if (response.status === "accepted") {
      return { ok: true, error: "" };
    }
    return { ok: false, error: "chain activity returned non-accepted response" };
  } catch (error) {
    return { ok: false, error: error.message };
  }
}

async function runWorker() {
  const cycleStartedAt = new Date().toISOString();
  console.log(`[${cycleStartedAt}] Starting payout worker cycle...`);

  try {
    ensureConfigExists();
    ensureQueueExists();
    ensureStateExists();

    const config = loadJson(CONFIG_PATH);
    const queue = loadJson(QUEUE_PATH);
    const state = loadJson(STATE_PATH);

    if (!Array.isArray(queue.items)) {
      throw new Error("Queue file must contain items array");
    }

    ensureStateShape(state);
    ensureHourlyWindow(state);

    const cycle = {
      cycle_started_at: cycleStartedAt,
      cycle_finished_at: "",
      dry_run: Boolean(config.dry_run),
      payouts_pending_seen: 0,
      payouts_attempted: 0,
      payouts_paid: 0,
      payouts_failed: 0,
      payouts_duplicate_blocked: 0,
      payouts_chain_activity_failed: 0,
      validation_invalid_request: 0,
      validation_wallet_not_found: 0,
      validation_wallet_not_eligible: 0,
      validation_reserve_blocked: 0,
      validation_per_user_cap_blocked: 0,
      validation_hourly_cap_blocked: 0,
      reserve_ratio: 0,
      reserve_ratio_min: Number(config.reserve_ratio_min || 0),
      hourly_cap_usdc: Number(config.max_total_payout_usdc_per_hour || 0),
      hourly_paid_total_usdc: 0,
      cap_usage_daily_top_users: [],
      dual_audit_coverage_percent: 0,
      dual_audit_success_count: 0,
      dual_audit_total_paid_count: 0,
      queue_transition_policy_ok: true,
      notes: [],
    };

    // Fully automatic mode: ingest payout candidates into queue each cycle.
    try {
      const ingest = await autoIngestQueue(config, queue, state);
      if (ingest.reason === "ok") {
        console.log(`Auto-ingest: added ${ingest.added}, skipped ${ingest.skipped}`);
      } else if (ingest.reason !== "disabled") {
        console.log(`Auto-ingest skipped: ${ingest.reason}`);
      }
    } catch (ingestError) {
      console.error(`Auto-ingest error: ${ingestError.message}`);
    }

    const maxRetries = Number(config.max_retries || 3);
    const hourlyMax = Number(config.max_total_payout_usdc_per_hour || 0);
    const perUserDailyMax = Number(config.max_payout_usdc_per_user_per_day || 0);
    const minSessions = Number(config.min_sessions || 0);
    const reserveRatioMin = Number(config.reserve_ratio_min || 0);
    const reserveRatio = getReserveRatio(config);
    cycle.reserve_ratio = reserveRatio;
    const nowIso = new Date().toISOString();
    let processedCount = 0;

    for (const item of queue.items) {
      if (item.status !== "pending") {
        continue;
      }

      cycle.payouts_pending_seen += 1;

      const requestId = String(item.request_id || "");
      const swapRef = String(item.swap_reference || "");
      const wallet = String(item.user_wallet || "").toUpperCase();
      const amount = Number(item.usdc_amount || 0);

      if (!requestId || !wallet || amount <= 0) {
        item.status = "failed";
        item.last_error = "invalid payout request fields";
        item.updated_at = nowIso;
        cycle.validation_invalid_request += 1;
        cycle.payouts_failed += 1;
        continue;
      }

      if (state.processed_request_ids.includes(requestId)) {
        item.status = "duplicate";
        item.last_error = "request_id already processed";
        item.updated_at = nowIso;
        cycle.payouts_duplicate_blocked += 1;
        continue;
      }

      if (swapRef && state.processed_swap_references.includes(swapRef)) {
        item.status = "duplicate";
        item.last_error = "swap_reference already processed";
        item.updated_at = nowIso;
        cycle.payouts_duplicate_blocked += 1;
        continue;
      }

      const testMode = Boolean(config.test_mode_skip_web2_eligibility);
      if (!testMode) {
        const eligibility = await getWeb2Eligibility(config.base_url, wallet);
        if (!eligibility.found) {
          item.last_error = "wallet not found in web2 ledger";
          item.updated_at = nowIso;
          cycle.validation_wallet_not_found += 1;
          continue;
        }
        if (eligibility.sessions < minSessions || !eligibility.is_eligible) {
          item.last_error = "wallet not eligible yet";
          item.updated_at = nowIso;
          cycle.validation_wallet_not_eligible += 1;
          continue;
        }
      }

      if (reserveRatio < reserveRatioMin) {
        item.last_error = "reserve ratio below minimum threshold";
        item.updated_at = nowIso;
        cycle.validation_reserve_blocked += 1;
        continue;
      }

      const userDayKey = getUserDayKey(wallet);
      const userPaidToday = Number(state.user_daily_paid_usdc[userDayKey] || 0);

      if (userPaidToday + amount > perUserDailyMax) {
        item.last_error = "daily per-user payout cap exceeded";
        item.updated_at = nowIso;
        cycle.validation_per_user_cap_blocked += 1;
        continue;
      }

      if (Number(state.hourly_paid_total_usdc || 0) + amount > hourlyMax) {
        item.last_error = "hourly total payout cap exceeded";
        item.updated_at = nowIso;
        cycle.validation_hourly_cap_blocked += 1;
        continue;
      }

      cycle.payouts_attempted += 1;

      if (Boolean(config.dry_run)) {
        item.status = "paid";
        item.payout_tx_hash = `dryrun-${requestId}`;
        item.paid_at = nowIso;
        item.updated_at = nowIso;
        cycle.payouts_paid += 1;
      } else {
        const result = await invokePayoutExecutor(config, item);
        if (!result.ok) {
          const nextRetries = Number(item.retries || 0) + 1;
          item.retries = nextRetries;
          item.last_error = result.error;
          item.updated_at = nowIso;
          if (nextRetries >= maxRetries) {
            item.status = "failed";
            cycle.payouts_failed += 1;
          }
          continue;
        }

        if (!result.tx_hash) {
          const nextRetries = Number(item.retries || 0) + 1;
          item.retries = nextRetries;
          item.last_error = "executor missing tx_hash in success response";
          item.updated_at = nowIso;
          if (nextRetries >= maxRetries) {
            item.status = "failed";
            cycle.payouts_failed += 1;
          }
          continue;
        }

        item.status = "paid";
        item.payout_tx_hash = result.tx_hash;
        item.paid_at = nowIso;
        item.updated_at = nowIso;
        cycle.payouts_paid += 1;

        const chainActivity = await invokeOnchainPayoutActivity(config, item, result.tx_hash);
        item.chain_activity_error = chainActivity.ok ? "" : chainActivity.error;
        if (!chainActivity.ok) {
          cycle.payouts_chain_activity_failed += 1;
        }
      }

      state.processed_request_ids.push(requestId);
      if (swapRef) {
        state.processed_swap_references.push(swapRef);
      }

      state.hourly_paid_total_usdc = Number(state.hourly_paid_total_usdc || 0) + amount;
      state.user_daily_paid_usdc[userDayKey] = userPaidToday + amount;
      processedCount += 1;
    }

    cycle.hourly_paid_total_usdc = Number(state.hourly_paid_total_usdc || 0);
    cycle.cap_usage_daily_top_users = buildTodayCapUsage(state);

    const paidItems = queue.items.filter((x) => x && x.status === "paid");
    const dualAuditSuccess = paidItems.filter((x) => {
      const hasTx = Boolean(String(x.payout_tx_hash || "").trim());
      const chainOk = !x.chain_activity_error;
      return hasTx && chainOk;
    }).length;
    cycle.dual_audit_total_paid_count = paidItems.length;
    cycle.dual_audit_success_count = dualAuditSuccess;
    cycle.dual_audit_coverage_percent =
      paidItems.length > 0 ? Number(((dualAuditSuccess / paidItems.length) * 100).toFixed(2)) : 100;

    const allowedStatuses = new Set(["pending", "paid", "failed", "duplicate"]);
    const hasInvalidStatus = queue.items.some((x) => !allowedStatuses.has(String(x.status || "")));
    cycle.queue_transition_policy_ok = !hasInvalidStatus;
    if (hasInvalidStatus) {
      cycle.notes.push("Queue contains non-policy status values.");
    }

    cycle.cycle_finished_at = new Date().toISOString();

    saveJson(QUEUE_PATH, queue);
    saveJson(STATE_PATH, state);
    saveOperationsCycle(OPERATIONS_REPORT_PATH, cycle);

    console.log(`Processed payout items: ${processedCount}`);
    console.log(`Hourly total paid (USDC): ${state.hourly_paid_total_usdc}`);
    console.log(`Reserve ratio: ${reserveRatio}`);
    console.log(`Saved operations report: ${OPERATIONS_REPORT_PATH}`);
    console.log(`[${new Date().toISOString()}] Payout worker cycle completed successfully`);
  } catch (error) {
    console.error(`[${new Date().toISOString()}] Worker execution error:`);
    if (String(error.message || "").includes("ENOENT") && String(error.message || "").includes("/var/data")) {
      console.error("Persistent disk path is not available. Verify Render Disk is attached and mounted at /var/data.");
    }
    console.error(error.stack || error.message);
  }
}

runWorker();

if (process.env.SCHEDULE_INTERVAL_MINUTES) {
  const interval = Number(process.env.SCHEDULE_INTERVAL_MINUTES) * 60 * 1000;
  if (Number.isFinite(interval) && interval > 0) {
    console.log(`Scheduling worker to run every ${interval / 1000 / 60} minutes`);
    setInterval(runWorker, interval);
  }
}
