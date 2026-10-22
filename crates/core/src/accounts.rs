use crate::vault::Vault;
use crate::{Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use crate::store::{self, Db};
use std::sync::Arc;

const PROVIDERS: &[&str] = &["anthropic", "openai", "gemini", "ollama"];

/// What the UI sees: never the key itself, only its last 4 characters.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub default_model: String,
    pub priority: i64,
    pub monthly_budget_usd: Option<f64>,
    pub status: String,
    pub key_hint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AccountInput {
    pub id: Option<String>,
    pub provider: String,
    pub label: String,
    pub default_model: String,
    pub priority: i64,
    pub monthly_budget_usd: Option<f64>,
    /// Required when creating; omitted on edit to keep the stored key.
    pub api_key: Option<String>,
}

pub struct AccountService<V: Vault> {
    db: Db,
    vault: Arc<V>,
}

impl<V: Vault> AccountService<V> {
    pub fn new(db: Connection, vault: V) -> Self {
        Self { db: store::shared(db), vault: Arc::new(vault) }
    }

    pub fn db(&self) -> Db {
        self.db.clone()
    }

    pub fn vault(&self) -> Arc<V> {
        self.vault.clone()
    }

    pub fn list(&self) -> Result<Vec<Account>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, provider, label, default_model, priority, monthly_budget_usd, status
             FROM accounts ORDER BY priority, label",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                provider: r.get(1)?,
                label: r.get(2)?,
                default_model: r.get(3)?,
                priority: r.get(4)?,
                monthly_budget_usd: r.get(5)?,
                status: r.get(6)?,
                key_hint: None,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let mut a = row?;
            a.key_hint = self.vault.get(&a.id)?.map(|k| hint(&k));
            out.push(a);
        }
        Ok(out)
    }

    pub fn upsert(&self, input: AccountInput) -> Result<Account> {
        validate(&input)?;
        let db = self.db.lock().unwrap();
        let existing = match &input.id {
            Some(id) => db
                .query_row("SELECT 1 FROM accounts WHERE id = ?1", [id], |_| Ok(()))
                .optional()?
                .is_some(),
            None => false,
        };
        let id = input.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let key = input.api_key.as_deref().filter(|k| !k.is_empty());
        if !existing && key.is_none() && input.provider != "ollama" {
            return Err(Error::Invalid("api_key is required for a new account".into()));
        }
        if let Some(k) = key {
            self.vault.set(&id, &SecretString::from(k.to_string()))?;
        }
        if existing {
            db.execute(
                "UPDATE accounts SET provider=?2, label=?3, default_model=?4, priority=?5,
                 monthly_budget_usd=?6 WHERE id=?1",
                params![id, input.provider, input.label, input.default_model, input.priority, input.monthly_budget_usd],
            )?;
        } else {
            db.execute(
                "INSERT INTO accounts (id, provider, label, default_model, priority, monthly_budget_usd)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![id, input.provider, input.label, input.default_model, input.priority, input.monthly_budget_usd],
            )?;
        }
        drop(db);
        self.list()?.into_iter().find(|a| a.id == id).ok_or(Error::NotFound)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        let n = db.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        if n == 0 {
            return Err(Error::NotFound);
        }
        self.vault.delete(id)
    }
}

fn hint(key: &SecretString) -> String {
    let k = key.expose_secret();
    let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

fn validate(i: &AccountInput) -> Result<()> {
    if !PROVIDERS.contains(&i.provider.as_str()) {
        return Err(Error::Invalid(format!("unknown provider '{}'", i.provider)));
    }
    if i.label.trim().is_empty() || i.default_model.trim().is_empty() {
        return Err(Error::Invalid("label and model are required".into()));
    }
    if matches!(i.monthly_budget_usd, Some(b) if b < 0.0) {
        return Err(Error::Invalid("budget must not be negative".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{store, vault::MemoryVault};

    fn svc() -> AccountService<MemoryVault> {
        AccountService::new(store::open_in_memory().unwrap(), MemoryVault::default())
    }

    fn input(label: &str, key: Option<&str>) -> AccountInput {
        AccountInput {
            id: None,
            provider: "anthropic".into(),
            label: label.into(),
            default_model: "model-x".into(),
            priority: 1,
            monthly_budget_usd: None,
            api_key: key.map(Into::into),
        }
    }

    #[test]
    fn create_masks_key_and_never_returns_it() {
        let s = svc();
        let a = s.upsert(input("Work", Some("sk-secret-1234"))).unwrap();
        assert_eq!(a.key_hint.as_deref(), Some("…1234"));
        assert!(!format!("{a:?}").contains("secret"));
    }

    #[test]
    fn edit_without_key_keeps_stored_key() {
        let s = svc();
        let a = s.upsert(input("Work", Some("sk-aaaa-9999"))).unwrap();
        let mut edit = input("Work renamed", None);
        edit.id = Some(a.id.clone());
        let b = s.upsert(edit).unwrap();
        assert_eq!(b.label, "Work renamed");
        assert_eq!(b.key_hint.as_deref(), Some("…9999"));
    }

    #[test]
    fn new_account_requires_key_except_ollama() {
        let s = svc();
        assert!(matches!(s.upsert(input("x", None)), Err(Error::Invalid(_))));
        let mut o = input("local", None);
        o.provider = "ollama".into();
        assert!(s.upsert(o).is_ok());
    }

    #[test]
    fn delete_removes_account_and_key() {
        let s = svc();
        let a = s.upsert(input("Work", Some("sk-1"))).unwrap();
        s.delete(&a.id).unwrap();
        assert!(s.list().unwrap().is_empty());
        assert!(matches!(s.delete(&a.id), Err(Error::NotFound)));
    }

    #[test]
    fn rejects_unknown_provider_and_negative_budget() {
        let s = svc();
        let mut i = input("x", Some("k"));
        i.provider = "nope".into();
        assert!(s.upsert(i).is_err());
        let mut j = input("x", Some("k"));
        j.monthly_budget_usd = Some(-1.0);
        assert!(s.upsert(j).is_err());
    }

    #[test]
    fn list_sorted_by_priority() {
        let s = svc();
        let mut a = input("B", Some("k"));
        a.priority = 2;
        let mut b = input("A", Some("k"));
        b.priority = 1;
        s.upsert(a).unwrap();
        s.upsert(b).unwrap();
        let labels: Vec<_> = s.list().unwrap().into_iter().map(|a| a.label).collect();
        assert_eq!(labels, ["A", "B"]);
    }
}
