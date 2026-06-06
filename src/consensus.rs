use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};

use crate::state::SharedState;

pub const DEFAULT_EPOCH_SECONDS: u64 = 2;
pub const DEFAULT_WORKER_SESSION_SECONDS: u64 = 21_600;

pub fn current_epoch_window(
    now: DateTime<Utc>,
    epoch_seconds: u64,
) -> (DateTime<Utc>, DateTime<Utc>) {
    // Defensive lower bound: a 0-second epoch would divide-by-zero in
    // rem_euclid. The startup validator (main.rs::validate_epoch_seconds)
    // already rejects this, but consensus.rs is a library boundary — never
    // trust the caller.
    let epoch_seconds = epoch_seconds.max(1) as i64;
    let now_ts = now.timestamp();
    let epoch_start_ts = now_ts - now_ts.rem_euclid(epoch_seconds);
    let epoch_end_ts = epoch_start_ts.saturating_add(epoch_seconds);

    // Fall back to `now` if the chrono conversion ever fails (e.g. a wildly
    // out-of-range timestamp on a misconfigured host). The block builder will
    // then close an empty window and try again on the next tick, instead of
    // panicking the consensus task and taking the whole node down.
    let start = Utc.timestamp_opt(epoch_start_ts, 0).single().unwrap_or(now);
    let end   = Utc.timestamp_opt(epoch_end_ts,   0).single().unwrap_or(now);
    (start, end)
}

pub async fn run_consensus(state: SharedState, epoch_seconds: u64) -> Result<()> {
    loop {
        let now = Utc::now();
        let (current_epoch_start, current_epoch_end) = current_epoch_window(now, epoch_seconds);

        {
            let mut state = state.write().await;
            if state.has_pending_block_work() && !state.has_block_in_epoch(current_epoch_start) {
                state
                    .create_block(current_epoch_start, current_epoch_end)
                    .await?;
            }
        }

        tokio::time::sleep(consensus_poll_interval(epoch_seconds)).await;
    }
}

fn consensus_poll_interval(epoch_seconds: u64) -> Duration {
    if epoch_seconds <= 2 {
        Duration::from_millis(200)
    } else if epoch_seconds <= 5 {
        Duration::from_millis(400)
    } else if epoch_seconds <= 30 {
        Duration::from_millis(800)
    } else {
        Duration::from_secs(1)
    }
}
