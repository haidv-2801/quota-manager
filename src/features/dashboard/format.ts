export function percentLeft(remaining: number | null, limit: number | null): number | null {
  if (remaining === null || limit === null || limit <= 0) return null;
  return Math.max(0, Math.min(100, Math.round((remaining / limit) * 100)));
}

export function formatReset(resetAt: number | null, now: number): string {
  if (resetAt === null) return "—";
  const secs = Math.max(0, Math.round((resetAt - now) / 1000));
  if (secs < 60) return `${secs} giây`;
  if (secs < 3600) return `${Math.round(secs / 60)} phút`;
  return `${Math.round(secs / 3600)} giờ`;
}

export const STATUS_LABEL: Record<string, string> = {
  healthy: "Bình thường",
  low: "Sắp hết",
  cooling_down: "Tạm nghỉ",
  exhausted: "Hết credit",
  disabled: "Key lỗi",
};
