import { useCallback, useEffect, useRef, useState } from "react";
import {
  createConversation, getMessages, listAccounts, listConversations, onChunk, pinAccount, sendMessage,
} from "../../ipc/api";
import type { Account, Conversation, StoredMessage } from "../../ipc/types";

export function ChatScreen() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [messages, setMessages] = useState<StoredMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [streaming, setStreaming] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const activeRef = useRef<string | null>(null);
  activeRef.current = activeId;

  const refreshConversations = useCallback(async () => {
    setConversations(await listConversations());
  }, []);

  const loadMessages = useCallback(async (id: string) => {
    setMessages(await getMessages(id));
  }, []);

  useEffect(() => {
    void refreshConversations();
    void listAccounts().then(setAccounts);
    const unlisten = onChunk((e) => {
      if (e.conversation_id === activeRef.current) setStreaming((s) => s + e.delta);
    });
    return () => { void unlisten.then((f) => f()); };
  }, [refreshConversations]);

  async function open(id: string) {
    setActiveId(id);
    setStreaming("");
    setError(null);
    await loadMessages(id);
  }

  async function newConversation() {
    const c = await createConversation();
    await refreshConversations();
    await open(c.id);
  }

  async function send(e: React.FormEvent) {
    e.preventDefault();
    if (!activeId || !draft.trim() || busy) return;
    const id = activeId;
    const text = draft;
    setDraft("");
    setBusy(true);
    setError(null);
    setStreaming("");
    // show the user's message immediately; it is already persisted by the backend before the request goes out
    setMessages((m) => [...m, { id: "pending", conversation_id: id, seq: m.length + 1, role: "user", text, account_id: null, model: null, created_at: Date.now() }]);
    try {
      await sendMessage(id, text);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
      setStreaming("");
      if (activeRef.current === id) await loadMessages(id);
      await refreshConversations();
    }
  }

  const active = conversations.find((c) => c.id === activeId);

  return (
    <div className="chat">
      <aside>
        <button onClick={() => void newConversation()}>Hội thoại mới</button>
        <ul>
          {conversations.map((c) => (
            <li key={c.id} className={c.id === activeId ? "active" : ""} onClick={() => void open(c.id)}>
              {c.title}
            </li>
          ))}
        </ul>
      </aside>
      <section>
        {!active && <p className="hint">Chọn hoặc tạo một hội thoại.</p>}
        {active && (
          <>
            <div className="row">
              <label>Tài khoản:{" "}
                <select
                  value={active.pinned_account_id ?? ""}
                  onChange={async (e) => {
                    await pinAccount(active.id, e.target.value || null);
                    await refreshConversations();
                  }}
                >
                  <option value="">Tự chọn theo độ ưu tiên</option>
                  {accounts.map((a) => <option key={a.id} value={a.id}>{a.label} ({a.provider})</option>)}
                </select>
              </label>
            </div>
            <div className="messages">
              {messages.map((m) => (
                <div key={m.id} className={`msg ${m.role}`}>
                  <pre>{m.text}</pre>
                  {m.model && <small>{m.model}</small>}
                </div>
              ))}
              {streaming && <div className="msg assistant"><pre>{streaming}</pre></div>}
            </div>
            {error && <p className="error">{error}</p>}
            <form onSubmit={send} className="row">
              <textarea value={draft} onChange={(e) => setDraft(e.target.value)} rows={3} placeholder="Nhập câu hỏi…" />
              <button type="submit" disabled={busy || !draft.trim()}>{busy ? "Đang trả lời…" : "Gửi"}</button>
            </form>
          </>
        )}
      </section>
    </div>
  );
}
