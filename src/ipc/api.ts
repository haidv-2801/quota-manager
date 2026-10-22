import { invoke } from "@tauri-apps/api/core";
import type { Account, AccountInput } from "./types";

export const listAccounts = () => invoke<Account[]>("list_accounts");
export const upsertAccount = (input: AccountInput) => invoke<Account>("upsert_account", { input });
export const deleteAccount = (accountId: string) => invoke<void>("delete_account", { accountId });
