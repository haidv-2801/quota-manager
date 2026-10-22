use crate::adapters::{self, ChatOutcome, ChatRequest, Provider, ProviderError, Usage};
use crate::message::{ContentBlock, Message, Role};
use crate::quota::QuotaService;
use crate::router::{self, Target};
use secrecy::SecretString;
use crate::store::Db;
use crate::vault::Vault;
use crate::{Error, Result};
use rusqlite::params;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

const MAX_TOKENS: u32 = 4096;
const DEFAULT_TITLE: &str = "Hội thoại mới";
/// Waits before the 1st and 2nd retry of a transient error on the same account.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

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

/// A switch the router made, shown in the chat as "đã chuyển từ X sang Y".
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SwitchRecord {
    pub at: i64,
    pub from_label: Option<String>,
    pub to_label: String,
    pub reason: String,
}

/// What `send` reports while it works.
pub enum ChatEvent<'a> {
    /// More reply text.
    Chunk(&'a str),
    /// The request moved to another account.
    Switched { from: String, to: String, reason: &'static str },
    /// Text already streamed is void; the reply starts over (after a switch or a retry).
    Restart,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error(transparent)]
    App(#[from] Error),
    #[error("{0}")]
    Provider(#[from] ProviderError),
    #[error("no usable account{}", recovery_hint(.retry_in_secs))]
    NoAccountAvailable { retry_in_secs: Option<u64> },
}

fn recovery_hint(secs: &Option<u64>) -> String {
    match secs {
        Some(s) => format!(" (an account recovers in about {s} s)"),
        None => "; add one or reset a disabled account".into(),
    }
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
    retry_delays: Vec<Duration>,
}

impl<V: Vault> ChatService<V> {
    pub fn new(db: Db, vault: Arc<V>) -> Self {
        Self::build(db, vault, Box::new(adapters::for_provider), RETRY_DELAYS.to_vec())
    }

    /// Custom provider factory with no waiting between retries; used by tests.
    pub fn with_factory(db: Db, vault: Arc<V>, factory: Factory) -> Self {
        Self::build(db, vault, factory, vec![Duration::ZERO; RETRY_DELAYS.len()])
    }

    fn build(db: Db, vault: Arc<V>, factory: Factory, retry_delays: Vec<Duration>) -> Self {
        let quota = QuotaService::new(db.clone());
        Self { db, vault, factory, quota, retry_delays }
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

    /// Stores the user message, then gets a reply from the best available account.
    ///
    /// - Transient errors are retried on the same account (delays 1 s, 3 s).
    /// - Rate limit, exhausted credit, or a context the account cannot hold: the request moves
    ///   to the next account, unless the conversation is pinned (then the error is returned).
    /// - A rejected key disables the account and is reported, not worked around.
    /// The user message stays stored whatever happens, so a retry never loses what was typed.
    /// When a reply is cut off by a failure, its partial text is discarded and the next account
    /// answers from scratch (`ChatEvent::Restart`); continuing from the partial is not done yet.
    pub async fn send(
        &self,
        conversation_id: &str,
        text: &str,
        on_event: &mut (dyn FnMut(ChatEvent<'_>) + Send),
    ) -> std::result::Result<StoredMessage, ChatError> {
        if text.trim().is_empty() {
            return Err(Error::Invalid("message is empty".into()).into());
        }
        self.quota.sweep(now_ms())?;
        let mut tried: Vec<String> = Vec::new();
        let mut current = self.pick(conversation_id, &tried)?.ok_or_else(|| self.no_account())?;
        self.append(conversation_id, Role::User, text, None, None)?;

        loop {
            let provider = (self.factory)(&current.provider)
                .ok_or_else(|| Error::Invalid(format!("provider '{}' is not supported yet", current.provider)))?;
            let key = self
                .vault
                .get(&current.id)?
                .ok_or_else(|| Error::Invalid(format!("no API key stored for '{}'", current.label)))?;
            let history = self
                .get_messages(conversation_id)?
                .into_iter()
                .filter_map(|m| Some(Message::text(Role::parse(&m.role)?, m.text)))
                .collect();
            let req = ChatRequest { model: current.model.clone(), messages: history, max_tokens: MAX_TOKENS };

            let err = match self.attempt(provider.as_ref(), &key, &req, on_event).await {
                Ok(out) => return self.finish(conversation_id, &current, out),
                Err(e) => e,
            };
            self.note_failure(&current.id, &err)?;
            let movable = matches!(
                err,
                ProviderError::RateLimited { .. } | ProviderError::CreditExhausted | ProviderError::ContextTooLong
            );
            if !movable || current.pinned {
                return Err(err.into());
            }
            tried.push(current.id.clone());
            let Some(next) = self.pick(conversation_id, &tried)? else {
                return Err(match err {
                    ProviderError::ContextTooLong => err.into(),
                    _ => self.no_account(),
                });
            };
            let reason = match err {
                ProviderError::RateLimited { .. } => "rate_limited",
                ProviderError::CreditExhausted => "credit_exhausted",
                _ => "context_too_long",
            };
            self.record_switch(conversation_id, &current.id, &next.id, reason)?;
            on_event(ChatEvent::Switched { from: current.label.clone(), to: next.label.clone(), reason });
            current = next;
        }
    }

    /// One account, with retries for transient errors.
    async fn attempt(
        &self,
        provider: &dyn Provider,
        key: &SecretString,
        req: &ChatRequest,
        on_event: &mut (dyn FnMut(ChatEvent<'_>) + Send),
    ) -> std::result::Result<ChatOutcome, ProviderError> {
        let mut delays = self.retry_delays.iter();
        let mut emitted = 0usize;
        loop {
            let result = {
                let mut forward = |s: &str| {
                    emitted += 1;
                    on_event(ChatEvent::Chunk(s));
                };
                provider.stream_chat(key, req, &mut forward).await
            };
            match (result, delays.next()) {
                (Err(ProviderError::Transient(_)), Some(wait)) => {
                    if emitted > 0 {
                        on_event(ChatEvent::Restart);
                        emitted = 0;
                    }
                    tokio::time::sleep(*wait).await;
                }
                (other, _) => {
                    if other.is_err() && emitted > 0 {
                        // the caller may switch account; whatever was streamed no longer counts
                        on_event(ChatEvent::Restart);
                    }
                    return other;
                }
            }
        }
    }

    fn finish(
        &self,
        conversation_id: &str,
        target: &Target,
        out: ChatOutcome,
    ) -> std::result::Result<StoredMessage, ChatError> {
        self.quota.record_snapshot(&target.id, &out.rate_limit, now_ms())?;
        let msg = self.append(conversation_id, Role::Assistant, &out.text, Some(&target.id), Some(&target.model))?;
        self.record_usage(&target.id, &msg.id, out.usage)?;
        Ok(msg)
    }

    fn pick(&self, conversation_id: &str, tried: &[String]) -> Result<Option<Target>> {
        router::pick(&self.db, conversation_id, tried)
    }

    fn no_account(&self) -> ChatError {
        let retry_in_secs = router::soonest_recovery_secs(&self.db, now_ms()).unwrap_or(None);
        ChatError::NoAccountAvailable { retry_in_secs }
    }

    fn record_switch(&self, conversation_id: &str, from: &str, to: &str, reason: &str) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT INTO switch_events (conversation_id, from_account_id, to_account_id, reason, at)
             VALUES (?1,?2,?3,?4,?5)",
            params![conversation_id, from, to, reason, now_ms()],
        )?;
        Ok(())
    }

    pub fn list_switches(&self, conversation_id: &str) -> Result<Vec<SwitchRecord>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT s.at, f.label, t.label, s.reason
             FROM switch_events s
             LEFT JOIN accounts f ON f.id = s.from_account_id
             LEFT JOIN accounts t ON t.id = s.to_account_id
             WHERE s.conversation_id = ?1 ORDER BY s.at, s.id",
        )?;
        let rows = stmt.query_map([conversation_id], |r| {
            Ok(SwitchRecord {
                at: r.get(0)?,
                from_label: r.get(1)?,
                to_label: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                reason: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
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
    use crate::adapters::{OnChunk, RateLimitInfo};
    use crate::vault::MemoryVault;
    use crate::{store, AccountInput, AccountService};
    use async_trait::async_trait;
    use secrecy::ExposeSecret;
    use std::collections::HashMap;
    use std::sync::Mutex;

    type Reply = std::result::Result<&'static str, ProviderError>;

    /// One scripted answer: optional text streamed first, then the result.
    #[derive(Clone)]
    struct Step {
        partial: Option<&'static str>,
        result: Reply,
    }

    fn ok(t: &'static str) -> Step {
        Step { partial: None, result: Ok(t) }
    }
    fn fail(e: ProviderError) -> Step {
        Step { partial: None, result: Err(e) }
    }
    fn fail_after(p: &'static str, e: ProviderError) -> Step {
        Step { partial: Some(p), result: Err(e) }
    }
    fn rate_limited() -> ProviderError {
        ProviderError::RateLimited { retry_after_secs: Some(30) }
    }

    /// Answers per API key; each call consumes one step, the last step repeats.
    #[derive(Default)]
    struct Script {
        steps: Mutex<HashMap<String, Vec<Step>>>,
        calls: Mutex<Vec<String>>,
        seen: Mutex<Vec<(String, Vec<Message>)>>,
    }

    struct Fake(Arc<Script>);

    #[async_trait]
    impl Provider for Fake {
        async fn stream_chat(
            &self,
            key: &SecretString,
            req: &ChatRequest,
            on_chunk: OnChunk<'_>,
        ) -> std::result::Result<ChatOutcome, ProviderError> {
            let key = key.expose_secret().to_string();
            self.0.calls.lock().unwrap().push(key.clone());
            self.0.seen.lock().unwrap().push((key.clone(), req.messages.clone()));
            let step = {
                let mut all = self.0.steps.lock().unwrap();
                let list = all.get_mut(&key).expect("no script for key");
                if list.len() > 1 { list.remove(0) } else { list[0].clone() }
            };
            if let Some(p) = step.partial {
                on_chunk(p);
            }
            let text = step.result?;
            on_chunk(text);
            Ok(ChatOutcome {
                text: text.into(),
                usage: Usage { input_tokens: 3, output_tokens: 4 },
                rate_limit: RateLimitInfo::default(),
            })
        }
    }

    struct Env {
        chat: ChatService<MemoryVault>,
        accounts: AccountService<MemoryVault>,
        script: Arc<Script>,
    }

    fn env(scripts: &[(&str, Vec<Step>)]) -> Env {
        let accounts = AccountService::new(store::open_in_memory().unwrap(), MemoryVault::default());
        let script = Arc::new(Script::default());
        for (key, steps) in scripts {
            script.steps.lock().unwrap().insert(key.to_string(), steps.clone());
        }
        let s2 = script.clone();
        let chat = ChatService::with_factory(
            accounts.db(),
            accounts.vault(),
            Box::new(move |_| Some(Box::new(Fake(s2.clone())))),
        );
        Env { chat, accounts, script }
    }

    /// The account's API key doubles as its script selector.
    fn add(env: &Env, label: &str, priority: i64) -> String {
        env.accounts
            .upsert(AccountInput {
                id: None,
                provider: "anthropic".into(),
                label: label.into(),
                default_model: "m".into(),
                priority,
                monthly_budget_usd: None,
                api_key: Some(format!("key-{label}")),
            })
            .unwrap()
            .id
    }

    fn status_of(env: &Env, label: &str) -> String {
        env.accounts.list().unwrap().into_iter().find(|a| a.label == label).unwrap().status
    }

    fn calls(env: &Env) -> Vec<String> {
        env.script.calls.lock().unwrap().clone()
    }

    #[derive(Debug, PartialEq)]
    enum Ev {
        Chunk(String),
        Switched(String, String, &'static str),
        Restart,
    }

    async fn send(env: &Env, conv: &str, text: &str) -> (std::result::Result<StoredMessage, ChatError>, Vec<Ev>) {
        let mut events = Vec::new();
        let r = env
            .chat
            .send(conv, text, &mut |e| {
                events.push(match e {
                    ChatEvent::Chunk(c) => Ev::Chunk(c.into()),
                    ChatEvent::Switched { from, to, reason } => Ev::Switched(from, to, reason),
                    ChatEvent::Restart => Ev::Restart,
                })
            })
            .await;
        (r, events)
    }

    #[tokio::test]
    async fn stores_both_messages_usage_and_sends_full_history() {
        let e = env(&[("key-A", vec![ok("pong")])]);
        let acc = add(&e, "A", 1);
        let conv = e.chat.create_conversation().unwrap();
        let (r, events) = send(&e, &conv.id, "ping").await;
        r.unwrap();
        send(&e, &conv.id, "again").await.0.unwrap();
        assert_eq!(events, [Ev::Chunk("pong".into())]);
        let msgs = e.chat.get_messages(&conv.id).unwrap();
        let roles: Vec<_> = msgs.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
        assert_eq!(roles, [("user", "ping"), ("assistant", "pong"), ("user", "again"), ("assistant", "pong")]);
        assert_eq!(msgs[1].account_id.as_deref(), Some(acc.as_str()));
        // the second request carried the whole conversation so far
        assert_eq!(e.script.seen.lock().unwrap()[1].1.len(), 3);
        let n: i64 = e
            .chat
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage_events WHERE input_tokens = 3", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(e.chat.list_conversations().unwrap()[0].title, "ping");
    }

    #[tokio::test]
    async fn rate_limit_moves_the_request_to_the_next_account() {
        let e = env(&[("key-A", vec![fail(rate_limited())]), ("key-B", vec![ok("from b")])]);
        add(&e, "A", 1);
        let b = add(&e, "B", 2);
        let conv = e.chat.create_conversation().unwrap();
        let (r, events) = send(&e, &conv.id, "hello").await;
        let reply = r.unwrap();
        assert_eq!((reply.text.as_str(), reply.account_id.as_deref()), ("from b", Some(b.as_str())));
        assert_eq!(events, [Ev::Switched("A".into(), "B".into(), "rate_limited"), Ev::Chunk("from b".into())]);
        assert_eq!(status_of(&e, "A"), "cooling_down");
        // B received the same history, with the user message once
        let seen = e.script.seen.lock().unwrap();
        assert_eq!(seen[1].0, "key-B");
        assert_eq!(seen[1].1, [Message::text(Role::User, "hello")]);
        drop(seen);
        // one user message and one reply are stored, and the switch is on record
        assert_eq!(e.chat.get_messages(&conv.id).unwrap().len(), 2);
        let sw = e.chat.list_switches(&conv.id).unwrap();
        assert_eq!(sw.len(), 1);
        assert_eq!((sw[0].from_label.as_deref(), sw[0].to_label.as_str(), sw[0].reason.as_str()), (Some("A"), "B", "rate_limited"));
        // the next request goes straight to B: A is cooling down
        send(&e, &conv.id, "next").await.0.unwrap();
        assert_eq!(calls(&e), ["key-A", "key-B", "key-B"]);
    }

    #[tokio::test]
    async fn exhausted_credit_switches_and_marks_the_account() {
        let e = env(&[("key-A", vec![fail(ProviderError::CreditExhausted)]), ("key-B", vec![ok("b")])]);
        add(&e, "A", 1);
        add(&e, "B", 2);
        let conv = e.chat.create_conversation().unwrap();
        let (r, events) = send(&e, &conv.id, "hi").await;
        r.unwrap();
        assert_eq!(events[0], Ev::Switched("A".into(), "B".into(), "credit_exhausted"));
        assert_eq!(status_of(&e, "A"), "exhausted");
    }

    #[tokio::test]
    async fn a_pinned_conversation_is_never_moved() {
        let e = env(&[("key-A", vec![fail(rate_limited())]), ("key-B", vec![ok("b")])]);
        let a = add(&e, "A", 2);
        add(&e, "B", 1);
        let conv = e.chat.create_conversation().unwrap();
        e.chat.pin_account(&conv.id, Some(&a)).unwrap();
        let (r, _) = send(&e, &conv.id, "hi").await;
        assert!(matches!(r.unwrap_err(), ChatError::Provider(ProviderError::RateLimited { .. })));
        assert_eq!(calls(&e), ["key-A"]);
        // the user message is kept for a retry
        assert_eq!(e.chat.get_messages(&conv.id).unwrap().len(), 1);
        // and with A cooling down, the next attempt reports it instead of using B
        let (r, _) = send(&e, &conv.id, "again").await;
        assert!(r.unwrap_err().to_string().contains("pinned"));
        assert_eq!(calls(&e), ["key-A"]);
    }

    #[tokio::test]
    async fn when_every_account_fails_the_user_hears_there_is_none_left() {
        let e = env(&[("key-A", vec![fail(rate_limited())]), ("key-B", vec![fail(ProviderError::CreditExhausted)])]);
        add(&e, "A", 1);
        add(&e, "B", 2);
        let conv = e.chat.create_conversation().unwrap();
        let (r, _) = send(&e, &conv.id, "hi").await;
        let err = r.unwrap_err();
        assert!(matches!(err, ChatError::NoAccountAvailable { retry_in_secs: Some(_) }), "{err:?}");
        assert!(err.to_string().contains("no usable account"));
        assert_eq!(e.chat.get_messages(&conv.id).unwrap().len(), 1);
        // asking again before anything recovers fails fast without calling a provider
        let before = calls(&e).len();
        assert!(send(&e, &conv.id, "again").await.0.is_err());
        assert_eq!(calls(&e).len(), before);
    }

    #[tokio::test]
    async fn a_rejected_key_is_reported_not_worked_around() {
        let e = env(&[("key-A", vec![fail(ProviderError::InvalidKey)]), ("key-B", vec![ok("b")])]);
        add(&e, "A", 1);
        add(&e, "B", 2);
        let conv = e.chat.create_conversation().unwrap();
        let (r, _) = send(&e, &conv.id, "hi").await;
        assert!(matches!(r.unwrap_err(), ChatError::Provider(ProviderError::InvalidKey)));
        assert_eq!(status_of(&e, "A"), "disabled");
        assert_eq!(calls(&e), ["key-A"]);
    }

    #[tokio::test]
    async fn transient_errors_retry_on_the_same_account() {
        let e = env(&[("key-A", vec![fail(ProviderError::Transient("net".into())), ok("fine")])]);
        add(&e, "A", 1);
        let conv = e.chat.create_conversation().unwrap();
        let (r, events) = send(&e, &conv.id, "hi").await;
        assert_eq!(r.unwrap().text, "fine");
        assert_eq!(calls(&e), ["key-A", "key-A"]);
        assert_eq!(events, [Ev::Chunk("fine".into())]);
        assert_eq!(status_of(&e, "A"), "healthy");
    }

    #[tokio::test]
    async fn transient_errors_give_up_after_two_retries_without_switching() {
        let e = env(&[("key-A", vec![fail(ProviderError::Transient("net".into()))]), ("key-B", vec![ok("b")])]);
        add(&e, "A", 1);
        add(&e, "B", 2);
        let conv = e.chat.create_conversation().unwrap();
        let (r, _) = send(&e, &conv.id, "hi").await;
        assert!(matches!(r.unwrap_err(), ChatError::Provider(ProviderError::Transient(_))));
        assert_eq!(calls(&e), ["key-A", "key-A", "key-A"]);
        assert_eq!(status_of(&e, "A"), "healthy");
    }

    #[tokio::test]
    async fn context_too_long_tries_a_roomier_account_without_penalising_the_first() {
        let e = env(&[("key-A", vec![fail(ProviderError::ContextTooLong)]), ("key-B", vec![ok("b")])]);
        add(&e, "A", 1);
        add(&e, "B", 2);
        let conv = e.chat.create_conversation().unwrap();
        let (r, events) = send(&e, &conv.id, "hi").await;
        r.unwrap();
        assert_eq!(events[0], Ev::Switched("A".into(), "B".into(), "context_too_long"));
        assert_eq!(status_of(&e, "A"), "healthy");
        // if nobody can hold it, the user gets the context error itself
        let e2 = env(&[("key-A", vec![fail(ProviderError::ContextTooLong)])]);
        add(&e2, "A", 1);
        let conv2 = e2.chat.create_conversation().unwrap();
        let (r, _) = send(&e2, &conv2.id, "hi").await;
        assert!(matches!(r.unwrap_err(), ChatError::Provider(ProviderError::ContextTooLong)));
    }

    #[tokio::test]
    async fn text_streamed_before_a_failure_is_voided() {
        let e = env(&[("key-A", vec![fail_after("par", rate_limited())]), ("key-B", vec![ok("whole")])]);
        add(&e, "A", 1);
        add(&e, "B", 2);
        let conv = e.chat.create_conversation().unwrap();
        let (r, events) = send(&e, &conv.id, "hi").await;
        assert_eq!(r.unwrap().text, "whole");
        assert_eq!(
            events,
            [
                Ev::Chunk("par".into()),
                Ev::Restart,
                Ev::Switched("A".into(), "B".into(), "rate_limited"),
                Ev::Chunk("whole".into()),
            ]
        );
        // only the complete reply is stored
        let msgs = e.chat.get_messages(&conv.id).unwrap();
        assert_eq!(msgs.last().unwrap().text, "whole");
    }

    #[tokio::test]
    async fn a_cooled_down_account_returns_after_its_time() {
        let e = env(&[("key-A", vec![ok("ok")])]);
        let id = add(&e, "A", 1);
        e.chat.quota().mark_rate_limited(&id, Some(0), now_ms() - 1).unwrap();
        let conv = e.chat.create_conversation().unwrap();
        // send() sweeps first, so an expired cool-down does not block the request
        assert!(send(&e, &conv.id, "hi").await.0.is_ok());
    }

    #[tokio::test]
    async fn errors_without_account_or_with_empty_text_store_nothing() {
        let e = env(&[]);
        let conv = e.chat.create_conversation().unwrap();
        assert!(matches!(send(&e, &conv.id, "x").await.0, Err(ChatError::NoAccountAvailable { .. })));
        assert!(send(&e, &conv.id, "   ").await.0.is_err());
        assert!(e.chat.get_messages(&conv.id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn successful_reply_keeps_status_when_no_limits_are_reported() {
        let e = env(&[("key-A", vec![ok("ok")])]);
        add(&e, "A", 1);
        let conv = e.chat.create_conversation().unwrap();
        send(&e, &conv.id, "hi").await.0.unwrap();
        let row = &e.chat.quota().dashboard(now_ms()).unwrap()[0];
        assert_eq!((row.status.as_str(), row.snapshot_at, row.tokens_used_today), ("healthy", None, 7));
    }
}
