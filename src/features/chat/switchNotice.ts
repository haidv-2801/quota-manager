import type { StoredMessage, SwitchRecord } from "../../ipc/types";

const REASON: Record<string, string> = {
  rate_limited: "bị giới hạn tốc độ",
  credit_exhausted: "hết credit",
  context_too_long: "hội thoại quá dài cho tài khoản trước",
  manual: "do bạn chọn",
};

export function switchText(from: string | null, to: string, reason: string): string {
  const why = REASON[reason] ?? reason;
  return `Đã chuyển từ ${from ?? "tài khoản trước"} sang ${to} (${why}). Câu trả lời có thể khác phong cách.`;
}

export type Row =
  | { kind: "message"; message: StoredMessage }
  | { kind: "switch"; key: string; text: string };

/** Interleaves messages and switch notices by time, so a notice sits before the reply it caused. */
export function buildRows(messages: StoredMessage[], switches: SwitchRecord[]): Row[] {
  const rows: Row[] = [];
  let s = 0;
  for (const m of messages) {
    // assistant replies are stored after the switch that produced them
    while (s < switches.length && switches[s].at <= m.created_at && m.role === "assistant") {
      const w = switches[s++];
      rows.push({ kind: "switch", key: `sw-${w.at}-${s}`, text: switchText(w.from_label, w.to_label, w.reason) });
    }
    rows.push({ kind: "message", message: m });
  }
  for (; s < switches.length; s++) {
    const w = switches[s];
    rows.push({ kind: "switch", key: `sw-${w.at}-${s}`, text: switchText(w.from_label, w.to_label, w.reason) });
  }
  return rows;
}
