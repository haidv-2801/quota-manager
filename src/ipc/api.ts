import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Account, AccountInput, AccountQuota, ChunkEvent, Conversation, RestartEvent, StoredMessage, SwitchedEvent, SwitchRecord,
} from "./types";

export const listAccounts = () => invoke<Account[]>("list_accounts");
export const upsertAccount = (input: AccountInput) => invoke<Account>("upsert_account", { input });
export const deleteAccount = (accountId: string) => invoke<void>("delete_account", { accountId });

export const createConversation = () => invoke<Conversation>("create_conversation");
export const listConversations = () => invoke<Conversation[]>("list_conversations");
export const getMessages = (conversationId: string) =>
  invoke<StoredMessage[]>("get_messages", { conversationId });
export const pinAccount = (conversationId: string, accountId: string | null) =>
  invoke<void>("pin_account", { conversationId, accountId });
export const sendMessage = (conversationId: string, text: string) =>
  invoke<StoredMessage>("send_message", { conversationId, text });

export const onChunk = (handler: (e: ChunkEvent) => void): Promise<UnlistenFn> =>
  listen<ChunkEvent>("chunk", (e) => handler(e.payload));

export const getQuotaDashboard = () => invoke<AccountQuota[]>("get_quota_dashboard");
export const resetAccountStatus = (accountId: string) => invoke<void>("reset_account_status", { accountId });
export const onQuotaChanged = (handler: (rows: AccountQuota[]) => void): Promise<UnlistenFn> =>
  listen<AccountQuota[]>("quota_changed", (e) => handler(e.payload));

export const listSwitches = (conversationId: string) =>
  invoke<SwitchRecord[]>("list_switches", { conversationId });
export const onSwitched = (handler: (e: SwitchedEvent) => void): Promise<UnlistenFn> =>
  listen<SwitchedEvent>("switched", (e) => handler(e.payload));
export const onRestart = (handler: (e: RestartEvent) => void): Promise<UnlistenFn> =>
  listen<RestartEvent>("restart", (e) => handler(e.payload));
