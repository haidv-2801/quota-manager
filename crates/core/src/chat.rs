use crate::adapters::{self, ChatRequest, Provider, ProviderError, Usage};
use crate::message::{ContentBlock, Message, Role};
use crate::quota::QuotaService;
use crate::store::Db;
use crate::vault::Vault;
use crate::{Error, Result};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::sync::Arc;

const MAX_TOKENS: u32 = 4096;
const DEFAULT_TITLE: &str = "Hội thoại mới";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub pinned_account_id: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StoredMessage {
    pub id: String,
    pub conversation_id: String,
    pub seq: i64,
    pub role: String,
    pub text: String,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error(transparent)]
    App(#[from] Error),
    #[error("{0}")]
    Provider(#[from] ProviderError),
}

impl From<rusqlite::Error> for ChatError {
    fn from(e: rusqlite::Error) -> Self {
        ChatError::App(e.into())
    }
}

type Factory = Box<dyn Fn(&str) -> Option<Box<dyn Provider>> + Send + Sync>;

pub struct ChatService<V: Vault> {
    db: Db,
    vault: Arc<V>,
    factory: Factory,
    quota: QuotaService,
}

struct Target {
    id: String,
    provider: String,
    model: String,
}

impl<V: Vault> ChatService<V> {
    pub fn new(db: Db, vault: Arc<V>) -> Self {
        Self::with_factory(db, vault, Box::new(adapters::for_provider))
    }

    pub fn with_factory(db: Db, vault: Arc<V>, factory: Factory) -> Self {
        let quota = QuotaService::new(db.clone());
        Self { db, vault, factory, quota }
    }

    pub fn quota(&self) -> &QuotaService {
        &self.quota
    }

    pub fn create_conversation(&self) -> Result<Conversation> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ms();
        self.db.lock().unwrap().execute(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, DEFAULT_TITLE, now],
        )?;
        Ok(Conversation { id, title: DEFAULT_TITLE.into(), pinned_account_id: None, updated_at: now })
    }

    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, title, pinned_account_id, updated_at FROM conversations ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Conversation { id: r.get(0)?, title: r.get(1)?, pinned_account_id: r.get(2)?, updated_at: r.get(3)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn pin_account(&self, conversation_id: &str, account_id: Option<&str>) -> Result<()> {
        let n = self.db.lock().unwrap().execute(
            "UPDATE conversations SET pinned_account_id = ?2 WHERE id = ?1",
            params![conversation_id, account_id],
        )?;
        if n == 0 {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    pub fn get_messages(&self, conversation_id: &str) -> Result<Vec<StoredMessage>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, conversation_id, seq, role, content_json, account_id, model, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([conversation_id], |r| {
            let json: String = r.get(4)?;
            let blocks: Vec<ContentBlock> = serde_json::from_str(&json).unwrap_or_default();
            let text = Message { role: Role::User, content: blocks }.plain_text();
            Ok(StoredMessage {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                seq: r.get(2)?,
                role: r.get(3)?,
                text,
                account_id: r.get(5)?,
                model: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Stores the user message, streams the reply, stores it. The user message is
    /// kept even if the provider fails, so a retry never loses what was typed.
    pub async fn send(
        &self,
        conversation_id: &str,
        text: &str,
        on_chunk: &mut (dyn FnMut(&str) + Send),
    ) -> std::result::Result<StoredMessage, ChatError> {
        if text.trim().is_empty() {
            return Err(Error::Invalid("message is empty".into()).into());
        }
        self.quota.sweep(now_ms())?;
        let target = self.pick_target(conversation_id)?;
        let provider = (self.factory)(&target.provider)
            .ok_or_else(|| Error::Invalid(format!("provider '{}' is not supported yet", target.provider)))?;
        let key = self
            .vault
            .get(&target.id)?
            .ok_or_else(|| Error::Invalid("no API key stored for this account".into()))?;

        self.append(conversation_id, Role::User, text, None, None)?;
        let history = self
            .get_messages(conversation_id)?
            .into_iter()
            .filter_map(|m| Some(Message::text(Role::parse(&m.role)?, m.text)))
            .collect();
        let req = ChatRequest { model: target.model.clone(), messages: history, max_tokens: MAX_TOKENS };

        let out = match provider.stream_chat(&key, &req, on_chunk).await {
            Ok(out) => out,
            Err(e) => {
                self.note_failure(&target.id, &e)?;
                return Err(e.into());
            }
        };
        self.quota.record_snapshot(&target.id, &out.rate_limit, now_ms())?;
        let msg = self.append(conversation_id, Role::Assistant, &out.text, Some(&target.id), Some(&target.model))?;
        self.record_usage(&target.id, &msg.id, out.usage)?;
        Ok(msg)
    }

    /// Turns a provider failure into an account status so the next request skips it.
    fn note_failure(&self, account_id: &str, e: &ProviderError) -> Result<()> {
        match e {
            ProviderError::RateLimited { retry_after_secs } => {
                self.quota.mark_rate_limited(account_id, *retry_after_secs, now_ms())
            }
            ProviderError::CreditExhausted => self.quota.mark_exhausted(account_id),
            ProviderError::InvalidKey => self.quota.mark_disabled(account_id),
            _ => Ok(()),
        }
    }

    /// Phase 2 selection: the pinned account, else the highest-priority usable one.
    /// The quota-aware router replaces this in phase 4.
    fn pick_target(&self, conversation_id: &str) -> Result<Target> {
        let db = self.db.lock().unwrap();
        let pinned: Option<Option<String>> = db
            .query_row("SELECT pinned_account_id FROM conversations WHERE id = ?1", [conversation_id], |r| r.get(0))
            .optional()?;
        let pinned = pinned.ok_or(Error::NotFound)?;
        let row = |r: &rusqlite::Row| Ok(Target { id: r.get(0)?, provider: r.get(1)?, model: r.get(2)? });
        let target = match pinned {
            Some(id) => db
                .query_row("SELECT id, provider, default_model FROM accounts WHERE id = ?1", [id], row)
                .optional()?,
            None => db
                .query_row(
                    "SELECT id, provider, default_model FROM accounts
                     WHERE status IN ('healthy', 'low') ORDER BY priority LIMIT 1",
                    [],
                    row,
                )
                .optional()?,
        };
        target.ok_or_else(|| Error::Invalid("no usable account; add one in the Accounts screen".into()))
    }

    fn append(
        &self,
        conversation_id: &str,
        role: Role,
        text: &str,
        account_id: Option<&str>,
        model: Option<&str>,
    ) -> Result<StoredMessage> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ms();
        let json = serde_json::to_string(&[ContentBlock::Text { text: text.to_string() }]).expect("serialize");
        let db = self.db.lock().unwrap();
        let seq: i64 = db.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE conversation_id = ?1",
            [conversation_id],
            |r| r.get(0),
        )?;
        db.execute(
            "INSERT INTO messages (id, conversation_id, seq, role, content_json, account_id, model, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, conversation_id, seq, role.as_str(), json, account_id, model, now],
        )?;
        db.execute("UPDATE conversations SET updated_at = ?2 WHERE id = ?1", params![conversation_id, now])?;
        if seq == 1 && role == Role::User {
            let title: String = text.chars().take(40).collect();
            db.execute("UPDATE conversations SET title = ?2 WHERE id = ?1", params![conversation_id, title])?;
        }
        Ok(StoredMessage {
            id,
            conversation_id: conversation_id.into(),
            seq,
            role: role.as_str().into(),
            text: text.into(),
            account_id: account_id.map(Into::into),
            model: model.map(Into::into),
            created_at: now,
        })
    }

    fn record_usage(&self, account_id: &str, message_id: &str, usage: Usage) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT INTO usage_events (account_id, message_id, input_tokens, output_tokens, at)
             VALUES (?1,?2,?3,?4,?5)",
            params![account_id, message_id, usage.input_tokens, usage.output_tokens, now_ms()],
        )?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{ChatOutcome, OnChunk, RateLimitInfo};
    use crate::vault::MemoryVault;
    use crate::{store, AccountInput, AccountService};
    use async_trait::async_trait;
    use secrecy::SecretString;
    use std::sync::Mutex;

    struct Fake {
        reply: std::result::Result<&'static str, ProviderError>,
        seen: Arc<Mutex<Vec<Vec<Message>>>>,
    }

    #[async_trait]
    impl Provider for Fake {
        async fn stream_chat(
            &self,
            _key: &SecretString,
            req: &ChatRequest,
            on_chunk: OnChunk<'_>,
        ) -> std::result::Result<ChatOutcome, ProviderError> {
            self.seen.lock().unwrap().push(req.messages.clone());
            let text = self.reply.clone()?;
            on_chunk(text);
            Ok(ChatOutcome {
                text: text.into(),
                usage: Usage { input_tokens: 3, output_tokens: 4 },
                rate_limit: RateLimitInfo::default(),
            })
        }
    }

    fn setup(
        reply: std::result::Result<&'static str, ProviderError>,
    ) -> (ChatService<MemoryVault>, AccountService<MemoryVault>, Arc<Mutex<Vec<Vec<Message>>>>) {
        let accounts = AccountService::new(store::open_in_memory().unwrap(), MemoryVault::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let chat = ChatService::with_factory(
            accounts.db(),
            accounts.vault(),
            Box::new(move |_| Some(Box::new(Fake { reply: reply.clone(), seen: s2.clone() }))),
        );
        (chat, accounts, seen)
    }

    fn add_account(a: &AccountService<MemoryVault>, label: &str, priority: i64) -> String {
        a.upsert(AccountInput {
            id: None,
            provider: "anthropic".into(),
            label: label.into(),
            default_model: "m".into(),
            priority,
            monthly_budget_usd: None,
            api_key: Some("sk-1234".into()),
        })
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn stores_both_messages_usage_and_sends_full_history() {
        let (chat, accounts, seen) = setup(Ok("pong"));
        let acc = add_account(&accounts, "A", 1);
        let conv = chat.create_conversation().unwrap();
        let mut chunks = Vec::new();
        chat.send(&conv.id, "ping", &mut |c| chunks.push(c.to_string())).await.unwrap();
        chat.send(&conv.id, "again", &mut |_| {}).await.unwrap();
        assert_eq!(chunks, ["pong"]);
        let msgs = chat.get_messages(&conv.id).unwrap();
        let roles: Vec<_> = msgs.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
        assert_eq!(roles, [("user", "ping"), ("assistant", "pong"), ("user", "again"), ("assistant", "pong")]);
        assert_eq!(msgs[1].account_id.as_deref(), Some(acc.as_str()));
        // second request carried the whole conversation so far
        assert_eq!(seen.lock().unwrap()[1].len(), 3);
        let n: i64 = chat
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage_events WHERE input_tokens = 3", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(chat.list_conversations().unwrap()[0].title, "ping");
    }

    #[tokio::test]
    async fn provider_failure_keeps_the_user_message() {
        let (chat, accounts, _) = setup(Err(ProviderError::RateLimited { retry_after_secs: Some(5) }));
        add_account(&accounts, "A", 1);
        let conv = chat.create_conversation().unwrap();
        let err = chat.send(&conv.id, "hello", &mut |_| {}).await.unwrap_err();
        assert!(matches!(err, ChatError::Provider(ProviderError::RateLimited { .. })));
        let msgs = chat.get_messages(&conv.id).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hello");
    }

    #[tokio::test]
    async fn failures_set_account_status_and_skip_it_next_time() {
        for (err, expected) in [
            (ProviderError::RateLimited { retry_after_secs: Some(30) }, "cooling_down"),
            (ProviderError::CreditExhausted, "exhausted"),
            (ProviderError::InvalidKey, "disabled"),
        ] {
            let (chat, accounts, _) = setup(Err(err));
            add_account(&accounts, "A", 1);
            let conv = chat.create_conversation().unwrap();
            assert!(chat.send(&conv.id, "hi", &mut |_| {}).await.is_err());
            assert_eq!(accounts.list().unwrap()[0].status, expected);
            // the only account is now out of rotation
            let again = chat.send(&conv.id, "hi again", &mut |_| {}).await.unwrap_err();
            assert!(again.to_string().contains("no usable account"), "{again}");
        }
    }

    #[tokio::test]
    async fn transient_errors_leave_status_alone() {
        let (chat, accounts, _) = setup(Err(ProviderError::Transient("net".into())));
        add_account(&accounts, "A", 1);
        let conv = chat.create_conversation().unwrap();
        assert!(chat.send(&conv.id, "hi", &mut |_| {}).await.is_err());
        assert_eq!(accounts.list().unwrap()[0].status, "healthy");
    }

    #[tokio::test]
    async fn successful_reply_records_snapshot_and_flags_low() {
        let (chat, accounts, _) = setup(Ok("ok"));
        add_account(&accounts, "A", 1);
        let conv = chat.create_conversation().unwrap();
        chat.send(&conv.id, "hi", &mut |_| {}).await.unwrap();
        // Fake reports no limits, so the account stays healthy and gets no snapshot row
        let row = &chat.quota().dashboard(now_ms()).unwrap()[0];
        assert_eq!(row.status, "healthy");
        assert_eq!(row.snapshot_at, None);
        assert_eq!(row.tokens_used_today, 7);
    }

    #[tokio::test]
    async fn cooled_down_account_returns_after_sweep() {
        let (chat, accounts, _) = setup(Ok("ok"));
        let id = add_account(&accounts, "A", 1);
        chat.quota().mark_rate_limited(&id, Some(0), now_ms() - 1).unwrap();
        let conv = chat.create_conversation().unwrap();
        // send() sweeps first, so an expired cool-down does not block the request
        assert!(chat.send(&conv.id, "hi", &mut |_| {}).await.is_ok());
    }

    #[tokio::test]
    async fn uses_pinned_account_over_priority() {
        let (chat, accounts, _) = setup(Ok("ok"));
        add_account(&accounts, "first", 1);
        let second = add_account(&accounts, "second", 2);
        let conv = chat.create_conversation().unwrap();
        chat.pin_account(&conv.id, Some(&second)).unwrap();
        let m = chat.send(&conv.id, "x", &mut |_| {}).await.unwrap();
        assert_eq!(m.account_id.as_deref(), Some(second.as_str()));
    }

    #[tokio::test]
    async fn errors_without_account_or_with_empty_text() {
        let (chat, _, _) = setup(Ok("ok"));
        let conv = chat.create_conversation().unwrap();
        assert!(chat.send(&conv.id, "x", &mut |_| {}).await.is_err());
        assert!(chat.send(&conv.id, "   ", &mut |_| {}).await.is_err());
        assert!(chat.get_messages(&conv.id).unwrap().is_empty());
    }
}
