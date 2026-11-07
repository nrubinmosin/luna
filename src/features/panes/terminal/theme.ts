import type { Terminal as XTerm } from '@xterm/xterm';
import { safely } from './safely';

export const dark = () => document.querySelector('[data-app]')?.getAttribute('data-theme') === 'dark';

export const TERM_FONT_SIZE = 14;
export const TERM_FONT_FAMILY = "'JetBrains Mono', 'Cascadia Mono', ui-monospace, monospace";

// A transparent background makes the webgl renderer blend every glyph against
// the pane, which is what washed the text out. Paint an opaque background that
// matches --term instead, and ship a full 16-color ANSI ramp — without one
// xterm falls back to its stock palette. Both ramps are the same hues at one
// lightness each, low in chroma: readable at 4.5:1 or better, never neon.
export const themeFor = (isDark: boolean) =>
  isDark
    ? {
        // Neutral graphite, no blue in it: a blue-black ground is what read
        // as haze. Text at ~13:1 rather than pure white's 19:1, which blooms.
        background: '#101010',
        foreground: '#d7d7d7',
        cursor: '#d9b56a',
        cursorAccent: '#101010',
        selectionBackground: '#494137',
        selectionForeground: '#f0f0f0',
        black: '#2a2a2a',
        red: '#e29d93',
        green: '#8fc090',
        yellow: '#caad71',
        blue: '#88b6e6',
        magenta: '#c9a0d3',
        cyan: '#6bc3bd',
        white: '#c8c8c8',
        brightBlack: '#8c8c8c',
        brightRed: '#f8b9af',
        brightGreen: '#add8ad',
        brightYellow: '#e1c792',
        brightBlue: '#a6cffa',
        brightMagenta: '#e1bbe9',
        brightCyan: '#90dbd5',
        brightWhite: '#ececec'
      }
    : {
        // Warm paper. Matches --term in styles/tokens.css.
        background: '#fdf2e2',
        foreground: '#26211a',
        cursor: '#8a5a14',
        cursorAccent: '#fdf2e2',
        selectionBackground: '#ddcfbc',
        selectionForeground: '#1c1812',
        black: '#26211a',
        red: '#9c443b',
        green: '#2f7434',
        yellow: '#825c00',
        blue: '#2266a4',
        magenta: '#814a8d',
        cyan: '#007872',
        white: '#d3cdc4',
        brightBlack: '#68625b',
        brightRed: '#862721',
        brightGreen: '#045e17',
        brightYellow: '#6d4400',
        brightBlue: '#004e90',
        brightMagenta: '#6b3078',
        brightCyan: '#00615d',
        brightWhite: '#1c1812'
      };

/** Keeps the terminal's colors on the app's theme as [data-app] switches it;
 *  returns the call that stops following. */
export function followTheme(term: XTerm): () => void {
  const mo = new MutationObserver(() => {
    safely(() => {
      term.options.theme = themeFor(dark());
    });
  });
  const appEl = document.querySelector('[data-app]');
  if (appEl) mo.observe(appEl, { attributes: true, attributeFilter: ['data-theme'] });
  return () => mo.disconnect();
}
