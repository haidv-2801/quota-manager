CREATE TABLE accounts (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  label TEXT NOT NULL,
  default_model TEXT NOT NULL,
  priority INTEGER NOT NULL,
  monthly_budget_usd REAL,
  status TEXT NOT NULL DEFAULT 'healthy',
  cooldown_until INTEGER
);
CREATE TABLE conversations (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  pinned_account_id TEXT REFERENCES accounts(id),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  role TEXT NOT NULL,
  content_json TEXT NOT NULL,
  account_id TEXT REFERENCES accounts(id),
  model TEXT,
  is_summary INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  UNIQUE (conversation_id, seq)
);
CREATE TABLE usage_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  message_id TEXT REFERENCES messages(id),
  input_tokens INTEGER NOT NULL,
  output_tokens INTEGER NOT NULL,
  cost_usd REAL,
  at INTEGER NOT NULL
);
CREATE TABLE quota_snapshots (
  account_id TEXT NOT NULL REFERENCES accounts(id),
  at INTEGER NOT NULL,
  requests_remaining INTEGER, requests_reset_at INTEGER,
  tokens_remaining INTEGER,   tokens_reset_at INTEGER,
  PRIMARY KEY (account_id, at)
);
CREATE TABLE switch_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  conversation_id TEXT NOT NULL REFERENCES conversations(id),
  from_account_id TEXT, to_account_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  at INTEGER NOT NULL
);
