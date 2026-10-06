/**
 * 消息元信息条的数字格式化（token 用量 / 花费）。
 */

/** token 数 → 「942」/「1.2k」/「12.3k」/「1.2M」；0/负数返回空串 */
export function fmtTokens(n: number | undefined | null): string {
  if (!n || n <= 0) return "";
  if (n < 1000) return String(n);
  if (n < 1_000_000) {
    const k = n / 1000;
    return `${k >= 100 ? Math.round(k) : k.toFixed(1).replace(/\.0$/, "")}k`;
  }
  const m = n / 1_000_000;
  return `${m.toFixed(1).replace(/\.0$/, "")}M`;
}

/** 成本 → 「$0.05」/「$0.0043」（小额保留 4 位有效）；0/负数返回空串 */
export function fmtCost(n: number | undefined | null): string {
  if (!n || n <= 0) return "";
  if (n >= 0.01) return `$${n.toFixed(2)}`;
  // 小额保留两位有效数字：0.00431 → "$0.0043"
  const digits = Math.min(6, Math.max(2, Math.ceil(-Math.log10(n)) + 1));
  return `$${n.toFixed(digits)}`;
}
