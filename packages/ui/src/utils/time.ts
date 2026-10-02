/** relative timestamps à la ZCode: 刚刚 / N分钟前 / N小时前 / 昨天 / N天前 / date */
export function relTime(iso: string | undefined | null): string {
  if (!iso) return "";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const diff = Date.now() - t;
  const min = 60_000, hour = 3_600_000, day = 86_400_000;
  if (diff < min) return "刚刚";
  if (diff < hour) return `${Math.floor(diff / min)}分钟前`;
  if (diff < day) return `${Math.floor(diff / hour)}小时前`;
  if (diff < 2 * day) return "昨天";
  if (diff < 30 * day) return `${Math.floor(diff / day)}天前`;
  const d = new Date(t);
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

export function basename(p: string | undefined): string {
  if (!p) return "";
  const norm = p.replace(/\\/g, "/");
  return norm.split("/").filter(Boolean).pop() ?? p;
}

export function greeting(): string {
  const h = new Date().getHours();
  if (h < 6) return "夜深了";
  if (h < 12) return "早上好";
  if (h < 14) return "中午好";
  if (h < 18) return "下午好";
  return "晚上好";
}

/** 毫秒 → 「N 秒」/「M 分 S 秒」/「H 小时 M 分」 */
export function formatSpan(ms: number): string {
  const s = Math.max(1, Math.round(ms / 1000));
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  if (m < 60) return r ? `${m} 分 ${r} 秒` : `${m} 分`;
  const h = Math.floor(m / 60);
  return `${h} 小时 ${m % 60} 分`;
}
