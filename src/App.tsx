import { useEffect, useRef, useState } from "react";
import { getQuotaDashboard, onQuotaChanged } from "./ipc/api";
import type { AccountQuota } from "./ipc/types";
import { AccountsScreen } from "./features/accounts/AccountsScreen";
import { ChatScreen } from "./features/chat/ChatScreen";
import { statusAlerts } from "./features/dashboard/alerts";
import { DashboardScreen } from "./features/dashboard/DashboardScreen";

type Tab = "chat" | "quota" | "accounts";

export function App() {
  const [tab, setTab] = useState<Tab>("chat");
  const [alerts, setAlerts] = useState<string[]>([]);
  const seen = useRef(new Map<string, string>());

  useEffect(() => {
    const apply = (rows: AccountQuota[]) => {
      const fresh = statusAlerts(seen.current, rows);
      seen.current = new Map(rows.map((r) => [r.account_id, r.status]));
      if (fresh.length) setAlerts((a) => [...a, ...fresh]);
    };
    void getQuotaDashboard().then((rows) => {
      seen.current = new Map(rows.map((r) => [r.account_id, r.status]));
    });
    const unlisten = onQuotaChanged(apply);
    return () => { void unlisten.then((f) => f()); };
  }, []);

  return (
    <>
      <nav className="tabs">
        <button className={tab === "chat" ? "on" : ""} onClick={() => setTab("chat")}>Chat</button>
        <button className={tab === "quota" ? "on" : ""} onClick={() => setTab("quota")}>Quota</button>
        <button className={tab === "accounts" ? "on" : ""} onClick={() => setTab("accounts")}>Tài khoản</button>
      </nav>
      {alerts.length > 0 && (
        <div className="alerts" role="alert">
          {alerts.map((a, i) => <p key={i}>{a}</p>)}
          <button onClick={() => setAlerts([])}>Đóng</button>
        </div>
      )}
      {tab === "chat" && <ChatScreen />}
      {tab === "quota" && <DashboardScreen />}
      {tab === "accounts" && <AccountsScreen />}
    </>
  );
}
