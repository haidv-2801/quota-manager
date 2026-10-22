import { useEffect, useState } from "react";
import { getQuotaDashboard, onQuotaChanged, resetAccountStatus } from "../../ipc/api";
import type { AccountQuota } from "../../ipc/types";
import { formatReset, percentLeft, STATUS_LABEL } from "./format";

function Bar({ label, remaining, limit }: { label: string; remaining: number | null; limit: number | null }) {
  const pct = percentLeft(remaining, limit);
  if (pct === null) return <div className="bar"><small>{label}: chưa có dữ liệu</small></div>;
  return (
    <div className="bar">
      <small>{label}: còn {remaining} / {limit} ({pct}%)</small>
      <progress max={100} value={pct} />
    </div>
  );
}

export function DashboardScreen() {
  const [rows, setRows] = useState<AccountQuota[]>([]);
  const [now, setNow] = useState(Date.now());
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getQuotaDashboard().then(setRows).catch((e) => setError(String(e)));
    const unlisten = onQuotaChanged(setRows);
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      clearInterval(tick);
      void unlisten.then((f) => f());
    };
  }, []);

  return (
    <main>
      <h1>Quota</h1>
      <p className="hint">
        Số còn lại lấy từ header của provider sau mỗi request. Token đã dùng là số đếm cục bộ của app (theo giờ UTC), không gồm request bạn gửi ngoài app.
      </p>
      {error && <p className="error">{error}</p>}
      {rows.length === 0 && <p>Chưa có tài khoản nào.</p>}
      {rows.map((r) => (
        <section key={r.account_id} className={`card status-${r.status}`}>
          <div className="row">
            <strong>{r.label} <small>({r.provider})</small></strong>
            <span className="pill">{STATUS_LABEL[r.status] ?? r.status}</span>
          </div>
          <Bar label="Request" remaining={r.requests_remaining} limit={r.requests_limit} />
          <Bar label="Token" remaining={r.tokens_remaining} limit={r.tokens_limit} />
          <small>
            Reset sau: {formatReset(r.status === "cooling_down" ? r.cooldown_until : r.resets_at, now)}
            {" · "}Đã dùng hôm nay: {r.tokens_used_today} token
            {" · "}Tháng này: {r.tokens_used_month} token
          </small>
          {(r.status === "exhausted" || r.status === "disabled") && (
            <div>
              <button onClick={() => void resetAccountStatus(r.account_id)}>
                Đặt lại trạng thái (sau khi nạp credit hoặc sửa key)
              </button>
            </div>
          )}
        </section>
      ))}
    </main>
  );
}
