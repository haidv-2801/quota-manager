import type { AccountQuota } from "../../ipc/types";

const WORSE = new Set(["low", "cooling_down", "exhausted", "disabled"]);

const TEXT: Record<string, string> = {
  low: "sắp hết quota",
  cooling_down: "bị giới hạn tốc độ, tạm nghỉ",
  exhausted: "hết credit",
  disabled: "key bị từ chối",
};

/** One message per account that just moved into a worse status. */
export function statusAlerts(prev: Map<string, string>, rows: AccountQuota[]): string[] {
  const out: string[] = [];
  for (const r of rows) {
    const before = prev.get(r.account_id);
    if (before !== undefined && before !== r.status && WORSE.has(r.status)) {
      out.push(`${r.label} (${r.provider}) ${TEXT[r.status]}`);
    }
  }
  return out;
}
