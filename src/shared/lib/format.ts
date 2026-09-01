export const tail2 = (p: string) => {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return {
    parent: parts.length > 1 ? parts[parts.length - 2] : '~',
    leaf: parts[parts.length - 1] || p
  };
};

export const clockTime = (d: Date) =>
  d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' });

export const clockDate = (d: Date) =>
  d.toLocaleDateString('en-US', { day: 'numeric', month: 'long' });

export const clockWeekday = (d: Date) =>
  d.toLocaleDateString('en-US', { weekday: 'long' });

// "2026-08-17T21:00:00+00:00" -> "41m" / "2h 12m" / "4d"
export const fmtReset = (iso: string | null): string => {
  if (!iso) return '—';
  const ms = new Date(iso).getTime() - Date.now();
  if (!Number.isFinite(ms) || ms <= 0) return 'now';
  const m = Math.round(ms / 60000);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 48) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  return `${Math.round(h / 24)}d`;
};

// "2026-08-17T21:00:00+00:00" -> "Aug 24, 21:00" in the system timezone.
export const fmtResetDate = (iso: string | null): string | null => {
  if (!iso) return null;
  const d = new Date(iso);
  if (!Number.isFinite(d.getTime())) return null;
  return d.toLocaleString(undefined, {
    day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit'
  });
};

// 1755463200000 -> "just now" / "42m ago" / "3h ago"; null -> "never".
export const agoLabel = (at: number | null) => {
  if (!at) return 'never';
  const m = Math.round((Date.now() - at) / 60000);
  if (m < 1) return 'just now';
  if (m < 60) return `${m}m ago`;
  return `${Math.round(m / 60)}h ago`;
};

export const limitColor = (v: number) => {
  if (v >= 0.85) return 'oklch(.63 .19 25)';
  if (v >= 0.6) return 'oklch(.78 .15 78)';
  return 'oklch(.64 .18 145)';
};

export const ACCENT = 'var(--dialog-blue)';

export const tint = (pct: number, base: string) =>
  `color-mix(in oklab, ${ACCENT} ${pct}%, ${base})`;
