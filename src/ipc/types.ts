// Mirrors quota_core::Account / AccountInput. Move to generated types (ts-rs) later.
export type Provider = "anthropic" | "openai" | "gemini" | "ollama";

export interface Account {
  id: string;
  provider: Provider;
  label: string;
  default_model: string;
  priority: number;
  monthly_budget_usd: number | null;
  status: string;
  key_hint: string | null;
}

export interface AccountInput {
  id?: string;
  provider: Provider;
  label: string;
  default_model: string;
  priority: number;
  monthly_budget_usd: number | null;
  api_key?: string;
}

export interface Conversation {
  id: string;
  title: string;
  pinned_account_id: string | null;
  updated_at: number;
}

export interface StoredMessage {
  id: string;
  conversation_id: string;
  seq: number;
  role: "user" | "assistant" | "system";
  text: string;
  account_id: string | null;
  model: string | null;
  created_at: number;
}

export interface ChunkEvent {
  conversation_id: string;
  delta: string;
}

export interface AccountQuota {
  account_id: string;
  label: string;
  provider: string;
  status: string;
  cooldown_until: number | null;
  snapshot_at: number | null;
  requests_remaining: number | null;
  requests_limit: number | null;
  tokens_remaining: number | null;
  tokens_limit: number | null;
  resets_at: number | null;
  tokens_used_today: number;
  tokens_used_month: number;
}

export interface SwitchRecord {
  at: number;
  from_label: string | null;
  to_label: string;
  reason: string;
}

export interface SwitchedEvent {
  conversation_id: string;
  from: string;
  to: string;
  reason: string;
}

export interface RestartEvent {
  conversation_id: string;
}
