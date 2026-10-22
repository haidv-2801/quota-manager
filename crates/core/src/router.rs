use crate::store::Db;
use crate::{Error, Result};
use rusqlite::{params, OptionalExtension};

/// The account a request goes to.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub model: String,
    /// True when the conversation is pinned to this account: the router must not move it elsewhere.
    pub pinned: bool,
}

/// Picks the account for the next attempt.
///
/// 1. A pinned conversation uses its account while it is `healthy` or `low`; if it is not,
///    that is an error for the user to resolve, never a silent switch.
/// 2. Otherwise the highest-priority `healthy` account, then the highest-priority `low` one.
///    Accounts in `excluded` (already tried for this request) are skipped.
/// Ordering is deterministic: no random choice and no round-robin.
pub fn pick(db: &Db, conversation_id: &str, excluded: &[String]) -> Result<Option<Target>> {
    let db = db.lock().unwrap();
    let pinned: Option<Option<String>> = db
        .query_row("SELECT pinned_account_id FROM conversations WHERE id = ?1", [conversation_id], |r| r.get(0))
        .optional()?;
    let pinned = pinned.ok_or(Error::NotFound)?;
    let row = |pinned: bool| {
        move |r: &rusqlite::Row| {
            Ok(Target { id: r.get(0)?, label: r.get(1)?, provider: r.get(2)?, model: r.get(3)?, pinned })
        }
    };
    const COLS: &str = "SELECT id, label, provider, default_model FROM accounts";

    if let Some(id) = pinned {
        let target = db
            .query_row(&format!("{COLS} WHERE id = ?1 AND status IN ('healthy', 'low')"), [&id], row(true))
            .optional()?;
        return match target {
            Some(t) if !excluded.contains(&t.id) => Ok(Some(t)),
            Some(_) => Ok(None),
            None => Err(Error::Invalid(
                "the account pinned to this conversation is unavailable; unpin it or wait for it to recover".into(),
            )),
        };
    }

    for status in ["healthy", "low"] {
        let mut stmt = db.prepare(&format!("{COLS} WHERE status = ?1 ORDER BY priority, label"))?;
        let rows = stmt.query_map(params![status], row(false))?;
        for t in rows {
            let t = t?;
            if !excluded.contains(&t.id) {
                return Ok(Some(t));
            }
        }
    }
    Ok(None)
}

/// Seconds until the first cooling-down account is usable again, if any.
pub fn soonest_recovery_secs(db: &Db, now: i64) -> Result<Option<u64>> {
    let until: Option<i64> = db.lock().unwrap().query_row(
        "SELECT MIN(cooldown_until) FROM accounts WHERE status = 'cooling_down' AND cooldown_until IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    Ok(until.map(|t| ((t - now).max(0) as u64 + 999) / 1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChatService;
    use crate::vault::MemoryVault;
    use crate::{store, AccountInput, AccountService};

    fn setup() -> (AccountService<MemoryVault>, String) {
        let accounts = AccountService::new(store::open_in_memory().unwrap(), MemoryVault::default());
        let chat = ChatService::new(accounts.db(), accounts.vault());
        let conv = chat.create_conversation().unwrap().id;
        (accounts, conv)
    }

    fn add(a: &AccountService<MemoryVault>, label: &str, priority: i64) -> String {
        a.upsert(AccountInput {
            id: None,
            provider: "anthropic".into(),
            label: label.into(),
            default_model: "m".into(),
            priority,
            monthly_budget_usd: None,
            api_key: Some("k".into()),
        })
        .unwrap()
        .id
    }

    fn set_status(a: &AccountService<MemoryVault>, id: &str, status: &str) {
        a.db().lock().unwrap().execute("UPDATE accounts SET status = ?2 WHERE id = ?1", params![id, status]).unwrap();
    }

    #[test]
    fn picks_by_priority_among_healthy() {
        let (a, conv) = setup();
        add(&a, "second", 2);
        let first = add(&a, "first", 1);
        assert_eq!(pick(&a.db(), &conv, &[]).unwrap().unwrap().id, first);
    }

    #[test]
    fn healthy_beats_low_even_with_worse_priority() {
        let (a, conv) = setup();
        let low = add(&a, "low-but-first", 1);
        let healthy = add(&a, "healthy", 2);
        set_status(&a, &low, "low");
        assert_eq!(pick(&a.db(), &conv, &[]).unwrap().unwrap().id, healthy);
        // with the healthy one excluded, the low one is the fallback
        assert_eq!(pick(&a.db(), &conv, &[healthy]).unwrap().unwrap().id, low);
    }

    #[test]
    fn skips_unusable_statuses_and_excluded_accounts() {
        let (a, conv) = setup();
        for (i, status) in ["cooling_down", "exhausted", "disabled"].into_iter().enumerate() {
            let id = add(&a, status, i as i64);
            set_status(&a, &id, status);
        }
        assert_eq!(pick(&a.db(), &conv, &[]).unwrap(), None);
        let ok = add(&a, "ok", 9);
        assert_eq!(pick(&a.db(), &conv, &[]).unwrap().unwrap().id, ok);
        assert_eq!(pick(&a.db(), &conv, &[ok]).unwrap(), None);
    }

    #[test]
    fn pinned_account_is_used_but_never_replaced_silently() {
        let (a, conv) = setup();
        let first = add(&a, "first", 1);
        let pinned = add(&a, "pinned", 2);
        let chat = ChatService::new(a.db(), a.vault());
        chat.pin_account(&conv, Some(&pinned)).unwrap();
        let t = pick(&a.db(), &conv, &[]).unwrap().unwrap();
        assert_eq!((t.id.as_str(), t.pinned), (pinned.as_str(), true));
        // once the pinned account is tried and failed, there is nothing else to try
        assert_eq!(pick(&a.db(), &conv, &[pinned.clone()]).unwrap(), None);
        // an unusable pinned account is an error, not a switch to `first`
        set_status(&a, &pinned, "cooling_down");
        assert!(pick(&a.db(), &conv, &[]).is_err());
        let _ = first;
    }

    #[test]
    fn soonest_recovery_rounds_up_to_whole_seconds() {
        let (a, _) = setup();
        let x = add(&a, "x", 1);
        let y = add(&a, "y", 2);
        assert_eq!(soonest_recovery_secs(&a.db(), 1_000).unwrap(), None);
        a.db().lock().unwrap().execute("UPDATE accounts SET status='cooling_down', cooldown_until=?2 WHERE id=?1", params![x, 11_500]).unwrap();
        a.db().lock().unwrap().execute("UPDATE accounts SET status='cooling_down', cooldown_until=?2 WHERE id=?1", params![y, 5_000]).unwrap();
        assert_eq!(soonest_recovery_secs(&a.db(), 1_000).unwrap(), Some(4));
    }
}
