/**
 * Preset accent colours a chat can be pinned to. The pane title bar wears the
 * colour as a Luna-style gloss and the sidebar row shows a stripe of it, so a
 * glance links the two without reading either name.
 *
 * Muted on purpose — moss, olive, slate, clay rather than signal colours: a
 * title bar is looked at all day, and only the waiting flash should shout.
 * Hand-picked per preset, low in saturation and dark enough for white title
 * text on every one. The keys are what chats store, so they stay as they were.
 */
export interface ChatColor {
  key: string;
  label: string;
  h: number;
  s: number;
  l: number;
}

export const CHAT_COLORS: ChatColor[] = [
  { key: 'sky', label: 'Slate blue', h: 212, s: 36, l: 43 },
  { key: 'teal', label: 'Deep teal', h: 184, s: 42, l: 29 },
  { key: 'green', label: 'Sage', h: 118, s: 22, l: 39 },
  { key: 'olive', label: 'Moss', h: 76, s: 38, l: 29 },
  { key: 'amber', label: 'Ochre', h: 42, s: 55, l: 37 },
  { key: 'orange', label: 'Clay', h: 22, s: 46, l: 43 },
  { key: 'red', label: 'Brick', h: 356, s: 38, l: 37 },
  { key: 'magenta', label: 'Plum', h: 318, s: 26, l: 35 },
  { key: 'purple', label: 'Dusk', h: 266, s: 26, l: 46 },
  { key: 'graphite', label: 'Graphite', h: 30, s: 4, l: 33 }
];

/**
 * A random colour for a newborn chat, dodging the ones already worn in its
 * group. Only once the whole palette is worn does chance run free; a colour
 * freed by a deleted chat is simply back in the pool next time.
 */
export const pickChatColor = (worn: Array<string | null | undefined>): string => {
  const taken = new Set(worn.filter(Boolean));
  const free = CHAT_COLORS.filter(c => !taken.has(c.key));
  const pool = free.length ? free : CHAT_COLORS;
  return pool[Math.floor(Math.random() * pool.length)].key;
};

export interface ChatColorTheme {
  /** The colour itself: the pane title bar mixes its gloss from it (theme.css),
   *  the sidebar row wears it as a stripe, the picker as a swatch. */
  swatch: string;
}

/** null for an unset (or unknown) key — the pane then keeps stock Luna blue. */
export function chatColorTheme(key: string | null | undefined): ChatColorTheme | null {
  const c = CHAT_COLORS.find(x => x.key === key);
  return c ? { swatch: `hsl(${c.h} ${c.s}% ${c.l}%)` } : null;
}
