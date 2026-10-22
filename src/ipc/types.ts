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
