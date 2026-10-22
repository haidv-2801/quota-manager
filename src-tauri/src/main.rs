#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use quota_core::chat::{ChatService, Conversation, StoredMessage};
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

/// Streams the reply as `chunk` events and returns the stored assistant message.
#[tauri::command]
async fn send_message(
    app: AppHandle,
    chat: State<'_, Chat>,
    conversation_id: String,
    text: String,
) -> Result<StoredMessage, String> {
    let id = conversation_id.clone();
    let mut on_chunk = move |delta: &str| {
        let _ = app.emit("chunk", Chunk { conversation_id: id.clone(), delta: delta.to_string() });
    };
    chat.send(&conversation_id, &text, &mut on_chunk).await.map_err(err)
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = store::open(&dir.join("app.db"))?;
            let accounts = AccountService::new(db, KeyringVault);
            app.manage(ChatService::new(accounts.db(), accounts.vault()));
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
            send_message
        ])
        .run(tauri::generate_context!())
        .expect("error while running app");
}
