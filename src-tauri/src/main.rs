#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use quota_core::{store, vault::KeyringVault, Account, AccountInput, AccountService};
use tauri::{Manager, State};

type Svc = AccountService<KeyringVault>;

fn err(e: quota_core::Error) -> String {
    e.to_string()
}

#[tauri::command]
fn list_accounts(svc: State<Svc>) -> Result<Vec<Account>, String> {
    svc.list().map_err(err)
}

#[tauri::command]
fn upsert_account(svc: State<Svc>, input: AccountInput) -> Result<Account, String> {
    svc.upsert(input).map_err(err)
}

#[tauri::command]
fn delete_account(svc: State<Svc>, account_id: String) -> Result<(), String> {
    svc.delete(&account_id).map_err(err)
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = store::open(&dir.join("app.db"))?;
            app.manage(AccountService::new(db, KeyringVault));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![list_accounts, upsert_account, delete_account])
        .run(tauri::generate_context!())
        .expect("error while running app");
}
