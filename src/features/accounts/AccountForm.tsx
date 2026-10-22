import { useState } from "react";
import type { Account, AccountInput, Provider } from "../../ipc/types";

const PROVIDERS: Provider[] = ["anthropic", "openai", "gemini", "ollama"];

interface Props {
  account?: Account;
  onSave: (input: AccountInput) => Promise<void>;
  onCancel: () => void;
}

export function AccountForm({ account, onSave, onCancel }: Props) {
  const [provider, setProvider] = useState<Provider>(account?.provider ?? "anthropic");
  const [label, setLabel] = useState(account?.label ?? "");
  const [model, setModel] = useState(account?.default_model ?? "");
  const [priority, setPriority] = useState(account?.priority ?? 1);
  const [budget, setBudget] = useState(account?.monthly_budget_usd?.toString() ?? "");
  const [apiKey, setApiKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await onSave({
        id: account?.id,
        provider,
        label,
        default_model: model,
        priority,
        monthly_budget_usd: budget === "" ? null : Number(budget),
        api_key: apiKey || undefined,
      });
      setApiKey("");
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <form onSubmit={submit} className="form">
      <label>Provider
        <select value={provider} onChange={(e) => setProvider(e.target.value as Provider)}>
          {PROVIDERS.map((p) => <option key={p}>{p}</option>)}
        </select>
      </label>
      <label>Nhãn
        <input value={label} onChange={(e) => setLabel(e.target.value)} required />
      </label>
      <label>Model mặc định
        <input value={model} onChange={(e) => setModel(e.target.value)} required />
      </label>
      <label>Độ ưu tiên (nhỏ = ưu tiên cao)
        <input type="number" value={priority} onChange={(e) => setPriority(Number(e.target.value))} />
      </label>
      <label>Ngân sách tháng (USD, để trống = không giới hạn)
        <input type="number" min="0" step="0.01" value={budget} onChange={(e) => setBudget(e.target.value)} />
      </label>
      <label>API key {account && "(để trống để giữ key hiện tại)"}
        <input type="password" autoComplete="off" value={apiKey} onChange={(e) => setApiKey(e.target.value)} />
      </label>
      {provider !== "ollama" && !account && (
        <p className="hint">Chỉ thêm tài khoản bạn được phép dùng. Kiểm tra điều khoản của provider nếu thêm tài khoản thứ hai cùng hãng.</p>
      )}
      {error && <p className="error">{error}</p>}
      <div className="row">
        <button type="submit">Lưu</button>
        <button type="button" onClick={onCancel}>Huỷ</button>
      </div>
    </form>
  );
}
