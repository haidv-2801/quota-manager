use crate::adapters::RateLimitInfo;
use crate::store::Db;
use crate::Result;
use chrono::Datelike;
use rusqlite::params;
use serde::Serialize;

/// An account drops to `low` when this fraction of its quota (or less) remains.
pub const LOW_THRESHOLD: f64 = 0.15;
/// Cool-down used when a 429 carries no `retry-after`.
pub const DEFAULT_COOLDOWN_SECS: u64 = 60;

/// Smallest remaining/limit ratio across the quotas the provider reported.
pub fn fraction_left(info: &RateLimitInfo) -> Option<f64> {
    let ratio = |remaining: Option<i64>, limit: Option<i64>| match (remaining, limit) {
        (Some(r), Some(l)) if l > 0 => Some((r.max(0) as f64) / (l as f64)),
        _ => None,
    };
    [ratio(info.requests_remaining, info.requests_limit), ratio(info.tokens_remaining, info.tokens_limit)]
        .into_iter()
        .flatten()
        .reduce(f64::min)
}

/// `healthy` or `low`, from what the provider last reported. No data counts as healthy.
pub fn status_for(info: &RateLimitInfo) -> &'static str {
    match fraction_left(info) {
        Some(f) if f <= LOW_THRESHOLD => "low",
        _ => "healthy",
    }
}

/// One row of the dashboard.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AccountQuota {
    pub account_id: String,
    pub label: String,
    pub provider: String,
    pub status: String,
    pub cooldown_until: Option<i64>,
    pub snapshot_at: Option<i64>,
    pub requests_remaining: Option<i64>,
    pub requests_limit: Option<i64>,
    pub tokens_remaining: Option<i64>,
    pub tokens_limit: Option<i64>,
    /// Earliest reset time the provider reported (unix ms).
    pub resets_at: Option<i64>,
    /// Tokens counted locally, UTC day and UTC month.
    pub tokens_used_today: i64,
    pub tokens_used_month: i64,
}

/// Owns every change to `accounts.status` after creation.
#[derive(Clone)]
pub struct QuotaService {
    db: Db,
}

impl QuotaService {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Stores the provider's latest numbers and re-evaluates healthy/low.
    /// `disabled` and `exhausted` accounts keep their status; only a person clears those.
    pub fn record_snapshot(&self, account_id: &str, info: &RateLimitInfo, now: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        if *info != RateLimitInfo::default() {
            db.execute(
                "INSERT OR REPLACE INTO quota_snapshots
                 (account_id, at, requests_remaining, requests_reset_at, tokens_remaining, tokens_reset_at,
                  requests_limit, tokens_limit)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    account_id, now, info.requests_remaining, info.requests_reset_at,
                    info.tokens_remaining, info.tokens_reset_at, info.requests_limit, info.tokens_limit
                ],
            )?;
        }
        db.execute(
            "UPDATE accounts SET status = ?2, cooldown_until = NULL
             WHERE id = ?1 AND status IN ('healthy', 'low', 'cooling_down')",
            params![account_id, status_for(info)],
        )?;
        Ok(())
    }

    /// 429: stop using the account until its reset time.
    pub fn mark_rate_limited(&self, account_id: &str, retry_after_secs: Option<u64>, now: i64) -> Result<()> {
        let until = now + (retry_after_secs.unwrap_or(DEFAULT_COOLDOWN_SECS) as i64) * 1000;
        self.set_status(account_id, "cooling_down", Some(until))
    }

    /// Credit/budget ran out: stays out of rotation until a person resets it.
    pub fn mark_exhausted(&self, account_id: &str) -> Result<()> {
        self.set_status(account_id, "exhausted", None)
    }

    /// Key rejected: stays out of rotation until the key is fixed and the status reset.
    pub fn mark_disabled(&self, account_id: &str) -> Result<()> {
        self.set_status(account_id, "disabled", None)
    }

    pub fn reset_status(&self, account_id: &str) -> Result<()> {
        self.set_status(account_id, "healthy", None)
    }

    fn set_status(&self, account_id: &str, status: &str, cooldown_until: Option<i64>) -> Result<()> {
        self.db.lock().unwrap().execute(
            "UPDATE accounts SET status = ?2, cooldown_until = ?3 WHERE id = ?1",
            params![account_id, status, cooldown_until],
        )?;
        Ok(())
    }

    /// Brings accounts whose cool-down has passed back to `healthy`. Returns how many.
    pub fn sweep(&self, now: i64) -> Result<usize> {
        Ok(self.db.lock().unwrap().execute(
            "UPDATE accounts SET status = 'healthy', cooldown_until = NULL
             WHERE status = 'cooling_down' AND cooldown_until IS NOT NULL AND cooldown_until <= ?1",
            [now],
        )?)
    }

    pub fn dashboard(&self, now: i64) -> Result<Vec<AccountQuota>> {
        let day_start = now - now.rem_euclid(86_400_000);
        let month_start = month_start_ms(now);
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT a.id, a.label, a.provider, a.status, a.cooldown_until,
                    s.at, s.requests_remaining, s.requests_limit, s.tokens_remaining, s.tokens_limit,
                    s.requests_reset_at, s.tokens_reset_at,
                    (SELECT COALESCE(SUM(input_tokens + output_tokens), 0) FROM usage_events
                       WHERE account_id = a.id AND at >= ?1),
                    (SELECT COALESCE(SUM(input_tokens + output_tokens), 0) FROM usage_events
                       WHERE account_id = a.id AND at >= ?2)
             FROM accounts a
             LEFT JOIN quota_snapshots s
               ON s.account_id = a.id
              AND s.at = (SELECT MAX(at) FROM quota_snapshots WHERE account_id = a.id)
             ORDER BY a.priority, a.label",
        )?;
        let rows = stmt.query_map(params![day_start, month_start], |r| {
            let req_reset: Option<i64> = r.get(10)?;
            let tok_reset: Option<i64> = r.get(11)?;
            Ok(AccountQuota {
                account_id: r.get(0)?,
                label: r.get(1)?,
                provider: r.get(2)?,
                status: r.get(3)?,
                cooldown_until: r.get(4)?,
                snapshot_at: r.get(5)?,
                requests_remaining: r.get(6)?,
                requests_limit: r.get(7)?,
                tokens_remaining: r.get(8)?,
                tokens_limit: r.get(9)?,
                resets_at: [req_reset, tok_reset].into_iter().flatten().min(),
                tokens_used_today: r.get(12)?,
                tokens_used_month: r.get(13)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

fn month_start_ms(now: i64) -> i64 {
    chrono::DateTime::from_timestamp_millis(now)
        .and_then(|d| d.date_naive().with_day(1))
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc().timestamp_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::MemoryVault;
    use crate::{store, AccountInput, AccountService};

    const DAY: i64 = 86_400_000;
    // 2026-03-15T12:00:00Z
    const NOW: i64 = 1_773_576_000_000;

    fn setup() -> (QuotaService, AccountService<MemoryVault>, String) {
        let accounts = AccountService::new(store::open_in_memory().unwrap(), MemoryVault::default());
        let id = accounts
            .upsert(AccountInput {
                id: None,
                provider: "anthropic".into(),
                label: "A".into(),
                default_model: "m".into(),
                priority: 1,
                monthly_budget_usd: None,
                api_key: Some("k".into()),
            })
            .unwrap()
            .id;
        (QuotaService::new(accounts.db()), accounts, id)
    }

    fn info(remaining: i64, limit: i64) -> RateLimitInfo {
        RateLimitInfo { tokens_remaining: Some(remaining), tokens_limit: Some(limit), ..Default::default() }
    }

    fn status(a: &AccountService<MemoryVault>) -> String {
        a.list().unwrap().remove(0).status
    }

    #[test]
    fn low_threshold_uses_the_scarcest_quota() {
        assert_eq!(status_for(&info(16, 100)), "healthy");
        assert_eq!(status_for(&info(15, 100)), "low");
        let mixed = RateLimitInfo {
            requests_remaining: Some(90),
            requests_limit: Some(100),
            tokens_remaining: Some(5),
            tokens_limit: Some(100),
            ..Default::default()
        };
        assert_eq!(status_for(&mixed), "low");
        assert_eq!(status_for(&RateLimitInfo::default()), "healthy");
        // remaining without a limit tells us nothing about the fraction
        let no_limit = RateLimitInfo { tokens_remaining: Some(1), ..Default::default() };
        assert_eq!(status_for(&no_limit), "healthy");
    }

    #[test]
    fn snapshot_moves_account_between_healthy_and_low() {
        let (q, a, id) = setup();
        q.record_snapshot(&id, &info(10, 100), NOW).unwrap();
        assert_eq!(status(&a), "low");
        q.record_snapshot(&id, &info(90, 100), NOW + 1).unwrap();
        assert_eq!(status(&a), "healthy");
    }

    #[test]
    fn snapshot_never_revives_disabled_or_exhausted() {
        let (q, a, id) = setup();
        q.mark_disabled(&id).unwrap();
        q.record_snapshot(&id, &info(90, 100), NOW).unwrap();
        assert_eq!(status(&a), "disabled");
        q.mark_exhausted(&id).unwrap();
        q.record_snapshot(&id, &info(90, 100), NOW + 1).unwrap();
        assert_eq!(status(&a), "exhausted");
        q.reset_status(&id).unwrap();
        assert_eq!(status(&a), "healthy");
    }

    #[test]
    fn rate_limit_cools_down_then_sweep_restores() {
        let (q, a, id) = setup();
        q.mark_rate_limited(&id, Some(30), NOW).unwrap();
        assert_eq!(status(&a), "cooling_down");
        assert_eq!(q.sweep(NOW + 29_999).unwrap(), 0);
        assert_eq!(status(&a), "cooling_down");
        assert_eq!(q.sweep(NOW + 30_000).unwrap(), 1);
        assert_eq!(status(&a), "healthy");
    }

    #[test]
    fn rate_limit_without_retry_after_uses_default_cooldown() {
        let (q, _, id) = setup();
        q.mark_rate_limited(&id, None, NOW).unwrap();
        let row = &q.dashboard(NOW).unwrap()[0];
        assert_eq!(row.cooldown_until, Some(NOW + (DEFAULT_COOLDOWN_SECS as i64) * 1000));
    }

    #[test]
    fn dashboard_shows_latest_snapshot_and_usage_windows() {
        let (q, a, id) = setup();
        let mut older = info(80, 100);
        older.tokens_reset_at = Some(NOW + 5_000);
        q.record_snapshot(&id, &older, NOW - 10).unwrap();
        let mut newer = info(40, 100);
        newer.requests_reset_at = Some(NOW + 2_000);
        newer.tokens_reset_at = Some(NOW + 9_000);
        q.record_snapshot(&id, &newer, NOW).unwrap();
        {
            let db = a.db();
            let db = db.lock().unwrap();
            for (at, tokens) in [(NOW - 1000, 10), (NOW - DAY, 100), (NOW - 20 * DAY, 1000)] {
                db.execute(
                    "INSERT INTO usage_events (account_id, input_tokens, output_tokens, at) VALUES (?1, ?2, 0, ?3)",
                    params![id, tokens, at],
                )
                .unwrap();
            }
        }
        let row = &q.dashboard(NOW).unwrap()[0];
        assert_eq!(row.tokens_remaining, Some(40));
        assert_eq!(row.resets_at, Some(NOW + 2_000));
        assert_eq!(row.tokens_used_today, 10);
        // NOW is mid-March: 14 days into the month, so the 20-day-old row falls outside it
        assert_eq!(row.tokens_used_month, 110);
    }

    #[test]
    fn dashboard_includes_accounts_without_snapshots() {
        let (q, _, _) = setup();
        let row = &q.dashboard(NOW).unwrap()[0];
        assert_eq!(row.snapshot_at, None);
        assert_eq!(row.tokens_used_today, 0);
    }
}
