import { useCallback, useEffect, useState } from "react";
import { deleteAccount, listAccounts, upsertAccount } from "../../ipc/api";
import type { Account, AccountInput } from "../../ipc/types";
import { AccountForm } from "./AccountForm";

export function AccountsScreen() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [editing, setEditing] = useState<Account | "new" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setAccounts(await listAccounts());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  async function save(input: AccountInput) {
    await upsertAccount(input);
    setEditing(null);
    await refresh();
  }

  async function remove(a: Account) {
    if (!window.confirm(`Xoá tài khoản "${a.label}" và key đã lưu?`)) return;
    try {
      await deleteAccount(a.id);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <main>
      <header className="row">
        <h1>Tài khoản AI</h1>
        <button onClick={() => setEditing("new")}>Thêm tài khoản</button>
      </header>
      {error && <p className="error">{error}</p>}
      {editing && (
        <AccountForm
          account={editing === "new" ? undefined : editing}
          onSave={save}
          onCancel={() => setEditing(null)}
        />
      )}
      <table>
        <thead>
          <tr><th>Ưu tiên</th><th>Nhãn</th><th>Provider</th><th>Model</th><th>Key</th><th>Trạng thái</th><th /></tr>
        </thead>
        <tbody>
          {accounts.map((a) => (
            <tr key={a.id}>
              <td>{a.priority}</td>
              <td>{a.label}</td>
              <td>{a.provider}</td>
              <td>{a.default_model}</td>
              <td>{a.key_hint ?? "—"}</td>
              <td>{a.status}</td>
              <td className="row">
                <button onClick={() => setEditing(a)}>Sửa</button>
                <button onClick={() => void remove(a)}>Xoá</button>
              </td>
            </tr>
          ))}
          {accounts.length === 0 && <tr><td colSpan={7}>Chưa có tài khoản nào.</td></tr>}
        </tbody>
      </table>
    </main>
  );
}
