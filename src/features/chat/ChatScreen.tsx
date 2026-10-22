import { useCallback, useEffect, useRef, useState } from "react";
import {
  createConversation, getMessages, listAccounts, listConversations, listSwitches, onChunk, onRestart,
  onSwitched, pinAccount, sendMessage,
} from "../../ipc/api";
import type { Account, Conversation, StoredMessage, SwitchRecord } from "../../ipc/types";
import { buildRows, switchText } from "./switchNotice";

export function ChatScreen() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [messages, setMessages] = useState<StoredMessage[]>([]);
  const [switches, setSwitches] = useState<SwitchRecord[]>([]);
  const [liveNotices, setLiveNotices] = useState<string[]>([]);
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
    const [m, sw] = await Promise.all([getMessages(id), listSwitches(id)]);
    setMessages(m);
    setSwitches(sw);
  }, []);

  useEffect(() => {
    void refreshConversations();
    void listAccounts().then(setAccounts);
    const unlisten = [
      onChunk((e) => {
        if (e.conversation_id === activeRef.current) setStreaming((s) => s + e.delta);
      }),
      // the reply starts over after a switch or retry: drop what was streamed so far
      onRestart((e) => {
        if (e.conversation_id === activeRef.current) setStreaming("");
      }),
      onSwitched((e) => {
        if (e.conversation_id === activeRef.current) {
          setLiveNotices((n) => [...n, switchText(e.from, e.to, e.reason)]);
        }
      }),
    ];
    return () => { unlisten.forEach((u) => void u.then((f) => f())); };
  }, [refreshConversations]);

  async function open(id: string) {
    setActiveId(id);
    setStreaming("");
    setLiveNotices([]);
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
    setLiveNotices([]);
    // show the user's message immediately; it is already persisted by the backend before the request goes out
    setMessages((m) => [...m, { id: "pending", conversation_id: id, seq: m.length + 1, role: "user", text, account_id: null, model: null, created_at: Date.now() }]);
    try {
      await sendMessage(id, text);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
      setStreaming("");
      if (activeRef.current === id) {
        await loadMessages(id);
        setLiveNotices([]); // now shown from the stored switch records
      }
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
              {buildRows(messages, switches).map((row) =>
                row.kind === "switch" ? (
                  <div key={row.key} className="notice">{row.text}</div>
                ) : (
                  <div key={row.message.id} className={`msg ${row.message.role}`}>
                    <pre>{row.message.text}</pre>
                    {row.message.model && <small>{row.message.model}</small>}
                  </div>
                ),
              )}
              {liveNotices.map((t, i) => <div key={`live-${i}`} className="notice">{t}</div>)}
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
