import React from "react";
import ReactDOM from "react-dom/client";
import { AccountsScreen } from "./features/accounts/AccountsScreen";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AccountsScreen />
  </React.StrictMode>,
);
