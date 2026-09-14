/**
 * A timestamp up to `iso` in the future by more than a few seconds is
 * either a real clock skew between this browser and the server, or a
 * genuinely bogus value — either way worth surfacing as "in Xm", not
 * silently flattened to "just now" alongside an actual few-seconds-ago
 * timestamp (which the magnitude check below would otherwise hide).
 */
export function relativeTime(iso: string | null): string {
  if (!iso) return "never";
  const diffMs = Date.now() - new Date(iso).getTime();
  const diffSec = Math.round(diffMs / 1000);
  if (Math.abs(diffSec) < 5) return "just now";
  const magnitude = magnitudeLabel(Math.abs(diffSec));
  return diffSec > 0 ? `${magnitude} ago` : `in ${magnitude}`;
}

function magnitudeLabel(diffSec: number): string {
  if (diffSec < 60) return `${diffSec}s`;
  const diffMin = Math.round(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m`;
  const diffHour = Math.round(diffMin / 60);
  if (diffHour < 24) return `${diffHour}h`;
  const diffDay = Math.round(diffHour / 24);
  return `${diffDay}d`;
}

export function formatDateTime(iso: string | null): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString();
}

export function triggerReasonLabel(reason: string): string {
  switch (reason) {
    case "latency_threshold_exceeded":
      return "Latency threshold exceeded";
    case "error_rate_threshold_exceeded":
      return "Error rate threshold exceeded";
    default:
      return reason;
  }
}
