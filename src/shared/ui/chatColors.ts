/**
 * Preset accent colours a chat can be pinned to. The pane title bar wears the
 * colour as a Luna-style gradient and the sidebar row shows a stripe of it, so
 * a glance links the two without reading either name.
 *
 * Hand-picked hue/sat/lightness per preset rather than a formula over hue
 * alone: warm hues (amber, orange) go glaring at the lightness that suits the
 * blues, and white title text has to stay readable on every one of them.
 */
export interface ChatColor {
  key: string;
  label: string;
  h: number;
  s: number;
  l: number;
}

export const CHAT_COLORS: ChatColor[] = [
  { key: 'sky', label: 'Sky', h: 200, s: 95, l: 45 },
  { key: 'teal', label: 'Teal', h: 175, s: 85, l: 36 },
  { key: 'green', label: 'Green', h: 135, s: 65, l: 38 },
  { key: 'olive', label: 'Olive', h: 80, s: 55, l: 36 },
  { key: 'amber', label: 'Amber', h: 38, s: 90, l: 42 },
  { key: 'orange', label: 'Orange', h: 22, s: 90, l: 46 },
  { key: 'red', label: 'Red', h: 3, s: 75, l: 44 },
  { key: 'magenta', label: 'Magenta', h: 325, s: 70, l: 42 },
  { key: 'purple', label: 'Purple', h: 275, s: 60, l: 46 },
  { key: 'graphite', label: 'Graphite', h: 220, s: 12, l: 40 }
];

export interface ChatColorTheme {
  /** Title-bar face, shaped like xp.css's own Luna gradient. */
  grad: string;
  /** The three frame edges xp.css paints in Luna blue. */
  top: string;
  right: string;
  /** Title text shadow — xp.css hardcodes a navy one. */
  shadow: string;
  /** Flat representative colour for swatches and the sidebar stripe. */
  swatch: string;
}

const hsl = (h: number, s: number, l: number) => `hsl(${(h + 360) % 360} ${s}% ${l}%)`;

/** null for an unset (or unknown) key — the pane then keeps stock Luna blue. */
export function chatColorTheme(key: string | null | undefined): ChatColorTheme | null {
  const c = CHAT_COLORS.find(x => x.key === key);
  if (!c) return null;
  const { h, s, l } = c;
  // Same stop layout as xp.css's title bar: bright lip, flat body, a lift
  // toward the bottom and a dark 1px seam under it.
  const mid = hsl(h, s, l);
  const hi = hsl(h, s, l + 4);
  return {
    grad: `linear-gradient(180deg, ${hsl(h - 12, s, l + 10)}, ${mid} 8%, ${hsl(h, s, l - 1)} 40%, ${hi} 88%, ${hi} 93%, ${mid} 95%, ${hsl(h + 3, s, l - 9)} 96%, ${hsl(h + 3, s, l - 9)})`,
    top: hsl(h, Math.min(100, s + 5), l - 6),
    right: hsl(h, Math.min(100, s + 5), l - 16),
    shadow: hsl(h + 15, Math.min(90, s + 5), 18),
    swatch: mid
  };
}
