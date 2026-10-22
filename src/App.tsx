import { useState } from "react";
import { AccountsScreen } from "./features/accounts/AccountsScreen";
import { ChatScreen } from "./features/chat/ChatScreen";

export function App() {
  const [tab, setTab] = useState<"chat" | "accounts">("chat");
  return (
    <>
      <nav className="tabs">
        <button className={tab === "chat" ? "on" : ""} onClick={() => setTab("chat")}>Chat</button>
        <button className={tab === "accounts" ? "on" : ""} onClick={() => setTab("accounts")}>Tài khoản</button>
      </nav>
      {tab === "chat" ? <ChatScreen /> : <AccountsScreen />}
    </>
  );
}
