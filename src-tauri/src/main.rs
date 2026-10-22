#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use quota_core::chat::{ChatEvent, ChatService, Conversation, StoredMessage, SwitchRecord};
use quota_core::quota::AccountQuota;
use quota_core::{store, vault::KeyringVault, Account, AccountInput, AccountService};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

type Accounts = AccountService<KeyringVault>;
type Chat = ChatService<KeyringVault>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Clone, Serialize)]
struct Chunk {
    conversation_id: String,
    delta: String,
}

#[derive(Clone, Serialize)]
struct Switched {
    conversation_id: String,
    from: String,
    to: String,
    reason: String,
}

#[derive(Clone, Serialize)]
struct Restart {
    conversation_id: String,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Tells the UI the quota picture changed; the UI diffs it to raise alerts.
fn emit_quota(app: &AppHandle, rows: Vec<AccountQuota>) {
    let _ = app.emit("quota_changed", rows);
}

#[tauri::command]
fn get_quota_dashboard(chat: State<Chat>) -> Result<Vec<AccountQuota>, String> {
    chat.quota().dashboard(now_ms()).map_err(err)
}

#[tauri::command]
fn reset_account_status(app: AppHandle, chat: State<Chat>, account_id: String) -> Result<(), String> {
    chat.quota().reset_status(&account_id).map_err(err)?;
    emit_quota(&app, chat.quota().dashboard(now_ms()).map_err(err)?);
    Ok(())
}

#[tauri::command]
fn list_accounts(svc: State<Accounts>) -> Result<Vec<Account>, String> {
    svc.list().map_err(err)
}

#[tauri::command]
fn upsert_account(svc: State<Accounts>, input: AccountInput) -> Result<Account, String> {
    svc.upsert(input).map_err(err)
}

#[tauri::command]
fn delete_account(svc: State<Accounts>, account_id: String) -> Result<(), String> {
    svc.delete(&account_id).map_err(err)
}

#[tauri::command]
fn create_conversation(chat: State<Chat>) -> Result<Conversation, String> {
    chat.create_conversation().map_err(err)
}

#[tauri::command]
fn list_conversations(chat: State<Chat>) -> Result<Vec<Conversation>, String> {
    chat.list_conversations().map_err(err)
}

#[tauri::command]
fn get_messages(chat: State<Chat>, conversation_id: String) -> Result<Vec<StoredMessage>, String> {
    chat.get_messages(&conversation_id).map_err(err)
}

#[tauri::command]
fn pin_account(chat: State<Chat>, conversation_id: String, account_id: Option<String>) -> Result<(), String> {
    chat.pin_account(&conversation_id, account_id.as_deref()).map_err(err)
}

#[tauri::command]
fn list_switches(chat: State<Chat>, conversation_id: String) -> Result<Vec<SwitchRecord>, String> {
    chat.list_switches(&conversation_id).map_err(err)
}

/// Streams the reply as `chunk` events (plus `switched` and `restart` when the router moves the
/// request to another account) and returns the stored assistant message.
#[tauri::command]
async fn send_message(
    app: AppHandle,
    chat: State<'_, Chat>,
    conversation_id: String,
    text: String,
) -> Result<StoredMessage, String> {
    let id = conversation_id.clone();
    let event_app = app.clone();
    let mut on_event = move |ev: ChatEvent| {
        let conversation_id = id.clone();
        let _ = match ev {
            ChatEvent::Chunk(delta) => event_app.emit("chunk", Chunk { conversation_id, delta: delta.to_string() }),
            ChatEvent::Switched { from, to, reason } => {
                event_app.emit("switched", Switched { conversation_id, from, to, reason: reason.to_string() })
            }
            ChatEvent::Restart => event_app.emit("restart", Restart { conversation_id }),
        };
    };
    let result = chat.send(&conversation_id, &text, &mut on_event).await.map_err(err);
    // success or failure, quota numbers and statuses may have moved
    if let Ok(rows) = chat.quota().dashboard(now_ms()) {
        emit_quota(&app, rows);
    }
    result
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = store::open(&dir.join("app.db"))?;
            let accounts = AccountService::new(db, KeyringVault);
            let chat = ChatService::new(accounts.db(), accounts.vault());

            // every 30 s: end expired cool-downs and refresh the dashboard
            let quota = chat.quota().clone();
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                let now = now_ms();
                if quota.sweep(now).is_ok() {
                    if let Ok(rows) = quota.dashboard(now) {
                        emit_quota(&handle, rows);
                    }
                }
            });

            app.manage(chat);
            app.manage(accounts);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            upsert_account,
            delete_account,
            create_conversation,
            list_conversations,
            get_messages,
            pin_account,
            list_switches,
            send_message,
            get_quota_dashboard,
            reset_account_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running app");
}
