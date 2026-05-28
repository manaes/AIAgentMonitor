export function formatTokensPerSec(v: number): string {
  if (v < 1) return "0";
  if (v < 1000) return v.toFixed(0);
  return (v / 1000).toFixed(1) + "k";
}

export function formatTokensTotal(n: number): string {
  if (n < 1000) return n.toString();
  if (n < 1_000_000) return (n / 1000).toFixed(1) + "k";
  return (n / 1_000_000).toFixed(2) + "M";
}

export function relativeTime(secsSinceEpoch: number): string {
  const elapsed = Math.floor(Date.now() / 1000) - secsSinceEpoch;
  if (elapsed < 5) return "just now";
  if (elapsed < 60) return `${elapsed}s ago`;
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
  return `${Math.floor(elapsed / 3600)}h ago`;
}

export function formatResetClock(secsSinceEpoch: number): string {
  const d = new Date(secsSinceEpoch * 1000);
  return d.toTimeString().slice(0, 5);
}
