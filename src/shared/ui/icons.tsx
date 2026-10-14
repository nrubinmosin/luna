import type { CSSProperties, ReactNode } from 'react';

/**
 * Luna's toolbar icons, all drawn to one brief so they read as a set: a 16px
 * grid, a dark outline one shade below the fill, a top-lit gradient, and the
 * colour carrying the meaning the way XP's own icons did — green to make,
 * red to stop, blue to arrange, yellow to edit. Unicode glyphs (↺ ✕ ⚙ «) stood
 * here before and came out at four different sizes in four different fonts.
 *
 * Gradients are referenced by id; each icon defines its own under a prefix,
 * so two icons on screen never pick up each other's.
 */
export type IconName =
  | 'plus' | 'pencil' | 'trash' | 'reset' | 'sun' | 'moon' | 'auto' | 'hide' | 'show'
  | 'even' | 'gear' | 'refresh' | 'accounts' | 'key' | 'power' | 'help' | 'folder'
  | 'agents' | 'sweep' | 'download' | 'warn' | 'update' | 'close' | 'check' | 'child' | 'claude' | 'codex'
  | 'recommend' | 'sync' | 'file';

const grad = (id: string, top: string, bottom: string) => (
  <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stopColor={top} />
    <stop offset="1" stopColor={bottom} />
  </linearGradient>
);

function draw(name: IconName, p: string): ReactNode {
  switch (name) {
    case 'plus':
      return (
        <>
          <defs>{grad(`${p}g`, '#7fd36a', '#2e8f1e')}</defs>
          <circle cx="8" cy="8" r="7" fill={`url(#${p}g)`} stroke="#1f6a12" />
          <path d="M8 4.2v7.6M4.2 8h7.6" stroke="#fff" strokeWidth="2.2" strokeLinecap="round" />
        </>
      );
    case 'pencil':
      return (
        <>
          <defs>{grad(`${p}g`, '#ffe27a', '#e3a41c')}</defs>
          <path d="M3.2 10.6 10.9 2.9l2.2 2.2-7.7 7.7-3 .8z" fill={`url(#${p}g)`} stroke="#7a5200" strokeLinejoin="round" />
          <path d="M10.9 2.9l1.1-1.1a1 1 0 0 1 1.4 0l.8.8a1 1 0 0 1 0 1.4l-1.1 1.1z" fill="#f08aa0" stroke="#7a2a3a" strokeLinejoin="round" />
          <path d="M3.2 10.6l2.2 2.2-3 .8z" fill="#f3e0c0" stroke="#7a5200" strokeLinejoin="round" />
          <path d="M2.4 13.6l.5-1.6 1.1 1.1z" fill="#333" />
        </>
      );
    case 'trash':
      return (
        <>
          <defs>{grad(`${p}g`, '#eef3fb', '#a9bad4')}</defs>
          <path d="M3.6 5h8.8l-.8 9.2a1 1 0 0 1-1 .8H5.4a1 1 0 0 1-1-.8z" fill={`url(#${p}g)`} stroke="#45597a" strokeLinejoin="round" />
          <rect x="2.5" y="2.8" width="11" height="2.2" rx=".6" fill="#d6e0ef" stroke="#45597a" />
          <path d="M6.3 2.8V1.8h3.4v1" fill="none" stroke="#45597a" />
          <path d="M6.2 7v6M8 7v6M9.8 7v6" stroke="#6d83a8" strokeWidth=".9" />
        </>
      );
    case 'reset':
    case 'refresh': {
      const [a, b, s] = name === 'reset' ? ['#8ec0ff', '#1f63c9', '#123f86'] : ['#86d873', '#2b8a1d', '#1c5c12'];
      return (
        <>
          <defs>{grad(`${p}g`, a, b)}</defs>
          <path
            d="M13.4 8A5.4 5.4 0 1 1 11.8 4.2L13.3 2.7v4.5H8.8l1.6-1.6A3.4 3.4 0 1 0 11.4 8z"
            fill={`url(#${p}g)`} stroke={s} strokeLinejoin="round"
          />
        </>
      );
    }
    case 'sun':
      return (
        <>
          <defs>{grad(`${p}g`, '#fff3a0', '#f5b400')}</defs>
          <path d="M8 .9v2M8 13.1v2M.9 8h2M13.1 8h2M3 3l1.4 1.4M11.6 11.6 13 13M13 3l-1.4 1.4M4.4 11.6 3 13" stroke="#d48a00" strokeWidth="1.4" strokeLinecap="round" />
          <circle cx="8" cy="8" r="3.6" fill={`url(#${p}g)`} stroke="#b06c00" />
        </>
      );
    case 'moon':
      return (
        <>
          <defs>{grad(`${p}g`, '#fff8e2', '#e3cf96')}</defs>
          <path d="M10.6 1.6A6.6 6.6 0 1 0 14.4 11 5.4 5.4 0 0 1 10.6 1.6z" fill={`url(#${p}g)`} stroke="#7b6a3a" strokeLinejoin="round" />
        </>
      );
    case 'auto':
      return (
        <>
          <defs>{grad(`${p}g`, '#fff3a0', '#f5b400')}</defs>
          <circle cx="8" cy="8" r="6.4" fill="#2b3550" stroke="#141a2c" />
          <path d="M8 1.6a6.4 6.4 0 0 1 0 12.8z" fill={`url(#${p}g)`} stroke="#b06c00" />
        </>
      );
    case 'hide':
    case 'show':
      return (
        <>
          <defs>{grad(`${p}g`, '#ffffff', '#c6d3f7')}</defs>
          <circle cx="8" cy="8" r="7" fill={`url(#${p}g)`} stroke="#7b93d0" />
          <path
            d={name === 'hide' ? 'M8.6 4.6 5.4 8l3.2 3.4M11.4 4.6 8.2 8l3.2 3.4' : 'M7.4 4.6 10.6 8l-3.2 3.4M4.6 4.6 7.8 8l-3.2 3.4'}
            fill="none" stroke="#215dc6" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"
          />
        </>
      );
    case 'even':
      return (
        <>
          <defs>{grad(`${p}g`, '#5aa0ff', '#1450c8')}</defs>
          <rect x="1.5" y="2" width="13" height="12" rx="1.4" fill="#fff" stroke="#1d3f86" />
          <rect x="1.5" y="2" width="13" height="2.6" rx="1.2" fill={`url(#${p}g)`} stroke="#1d3f86" />
          <path d="M8 4.6V14M1.5 9.3h13" stroke="#1d3f86" />
          <path d="M5.6 7.3 4 9.3l1.6 2M10.4 7.3l1.6 2-1.6 2" fill="none" stroke="#2a8a1d" strokeWidth="1.1" strokeLinecap="round" strokeLinejoin="round" />
        </>
      );
    case 'gear':
      return (
        <>
          <defs>{grad(`${p}g`, '#f2f2f2', '#9da3ad')}</defs>
          <path
            d="M6.9 1.2h2.2l.4 1.9 1.3.6 1.7-1.1 1.6 1.6-1.1 1.7.6 1.3 1.9.4v2.2l-1.9.4-.6 1.3 1.1 1.7-1.6 1.6-1.7-1.1-1.3.6-.4 1.9H6.9l-.4-1.9-1.3-.6-1.7 1.1-1.6-1.6 1.1-1.7-.6-1.3-1.9-.4V6.9l1.9-.4.6-1.3-1.1-1.7 1.6-1.6 1.7 1.1 1.3-.6z"
            fill={`url(#${p}g)`} stroke="#4c525c" strokeLinejoin="round"
          />
          <circle cx="8" cy="8" r="2.2" fill="#fff" stroke="#4c525c" />
        </>
      );
    case 'accounts':
      return (
        <>
          <defs>
            {grad(`${p}a`, '#8ec0ff', '#1f63c9')}
            {grad(`${p}b`, '#86d873', '#2b8a1d')}
          </defs>
          <circle cx="10.8" cy="4.6" r="2.4" fill="#f6d7b8" stroke="#8a5a3a" />
          <path d="M6.6 13.5a4.2 4.2 0 0 1 8.4 0z" fill={`url(#${p}b)`} stroke="#1c5c12" strokeLinejoin="round" />
          <circle cx="5.4" cy="5.6" r="2.6" fill="#f6d7b8" stroke="#8a5a3a" />
          <path d="M.8 15a4.6 4.6 0 0 1 9.2 0z" fill={`url(#${p}a)`} stroke="#123f86" strokeLinejoin="round" />
        </>
      );
    case 'key':
      return (
        <>
          <defs>{grad(`${p}g`, '#ffe27a', '#d69a0c')}</defs>
          <circle cx="5" cy="6" r="3.6" fill={`url(#${p}g)`} stroke="#7a5200" />
          <circle cx="4.2" cy="5.2" r="1.1" fill="#fff" stroke="#7a5200" strokeWidth=".8" />
          <path d="M7.6 8.6 13.8 14.8M11.2 12.2l1.5-1.5M12.8 13.8l1.2-1.2" stroke="#7a5200" strokeWidth="1.6" strokeLinecap="round" />
        </>
      );
    case 'power':
      return (
        <>
          <defs>{grad(`${p}g`, '#ffb08a', '#d4370f')}</defs>
          <circle cx="8" cy="8" r="7" fill={`url(#${p}g)`} stroke="#7e1f06" />
          <path d="M5.4 5.4a3.8 3.8 0 1 0 5.2 0" fill="none" stroke="#fff" strokeWidth="1.6" strokeLinecap="round" />
          <path d="M8 3.4v4.4" stroke="#fff" strokeWidth="1.6" strokeLinecap="round" />
        </>
      );
    case 'help':
      return (
        <>
          <defs>{grad(`${p}g`, '#8ec0ff', '#1f63c9')}</defs>
          <circle cx="8" cy="8" r="7" fill={`url(#${p}g)`} stroke="#123f86" />
          <path d="M5.9 6.1a2.1 2.1 0 1 1 3 1.9c-.6.3-.9.7-.9 1.4v.4" fill="none" stroke="#fff" strokeWidth="1.6" strokeLinecap="round" />
          <circle cx="8" cy="12" r="1" fill="#fff" />
        </>
      );
    case 'folder':
      return (
        <>
          <defs>{grad(`${p}g`, '#ffe9a0', '#f0c040')}</defs>
          <path d="M1.5 3.5h4.6l1.4 1.5h7v8.5h-13z" fill="#e8b830" stroke="#8a6400" strokeLinejoin="round" />
          <path d="M1.5 6.5h13v7h-13z" fill={`url(#${p}g)`} stroke="#8a6400" strokeLinejoin="round" />
        </>
      );
    case 'agents':
      return (
        <>
          <defs>{grad(`${p}g`, '#c9b6ff', '#6a3fd0')}</defs>
          <rect x="3" y="4.5" width="10" height="8.5" rx="2" fill={`url(#${p}g)`} stroke="#3a1f86" />
          <path d="M8 4.5V2.4" stroke="#3a1f86" />
          <circle cx="8" cy="2" r="1" fill="#f0a04b" stroke="#7a4a10" strokeWidth=".7" />
          <circle cx="6" cy="8.4" r="1.1" fill="#fff" />
          <circle cx="10" cy="8.4" r="1.1" fill="#fff" />
          <path d="M6 11h4" stroke="#fff" strokeLinecap="round" />
        </>
      );
    case 'sweep':
      return (
        <>
          <path d="M12.8 1.6 8.2 8.4" stroke="#8a5a2a" strokeWidth="1.6" strokeLinecap="round" />
          <path d="M6.2 7.2l3.6 2.4-1.9 5.2c-1.8-.2-4.4-1.9-5.6-3.5z" fill="#f0c040" stroke="#8a6400" strokeLinejoin="round" />
          <path d="M5.2 10.2l2 1.3M4.4 11.6l1.8 1.3" stroke="#8a6400" strokeWidth=".8" />
        </>
      );
    case 'update':
      return (
        <>
          <defs>{grad(`${p}g`, '#86d873', '#2b8a1d')}</defs>
          <path d="M8 1.5 12.8 6.6H10v5.4H6V6.6H3.2z" fill={`url(#${p}g)`} stroke="#1c5c12" strokeLinejoin="round" />
          <path d="M2 14.5h12" stroke="#7a7062" strokeWidth="1.6" strokeLinecap="round" />
        </>
      );
    case 'close':
      return (
        <>
          <defs>{grad(`${p}g`, '#f0a694', '#c8401c')}</defs>
          <circle cx="8" cy="8" r="6.6" fill={`url(#${p}g)`} stroke="#7e1f06" />
          <path d="M5.6 5.6l4.8 4.8M10.4 5.6l-4.8 4.8" stroke="#fff" strokeWidth="1.7" strokeLinecap="round" />
        </>
      );
    case 'check':
      return (
        <>
          <defs>{grad(`${p}g`, '#86d873', '#2b8a1d')}</defs>
          <circle cx="8" cy="8" r="6.6" fill={`url(#${p}g)`} stroke="#1c5c12" />
          <path d="M4.9 8.2l2.1 2.1 4.1-4.4" fill="none" stroke="#fff" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </>
      );
    case 'child':
      return (
        <path d="M4.5 2.5v6.2a1.8 1.8 0 0 0 1.8 1.8h6M10 8l2.6 2.5L10 13" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" opacity=".7" />
      );
    // The provider marks are drawn in the colour of the text around them:
    // they say which CLI, and should not pull the eye any more than its name.
    case 'claude': {
      // Claude: a starburst.
      const rays = Array.from({ length: 12 }, (_, i) => {
        const a = (i * Math.PI) / 6;
        const r = i % 2 ? 4.6 : 6.6;
        return `M8 8L${(8 + Math.cos(a) * r).toFixed(2)} ${(8 + Math.sin(a) * r).toFixed(2)}`;
      }).join('');
      return <path d={rays} stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />;
    }
    case 'codex':
      // OpenAI's knot: three rounded links, a third of a turn apart.
      return (
        <g fill="none" stroke="currentColor" strokeWidth="1.3">
          {[0, 60, 120].map(a => (
            <rect key={a} x="5.4" y="1.6" width="5.2" height="12.8" rx="2.6" transform={`rotate(${a} 8 8)`} />
          ))}
        </g>
      );
    case 'recommend':
      return (
        <>
          <defs>{grad(`${p}g`, '#fff3a0', '#f0b400')}</defs>
          <path d="M8 1.4l1.9 4 4.4.5-3.3 3 .9 4.4L8 11.1l-3.9 2.2.9-4.4-3.3-3 4.4-.5z" fill={`url(#${p}g)`} stroke="#8a6400" strokeLinejoin="round" />
        </>
      );
    case 'sync':
      return (
        <>
          <defs>{grad(`${p}g`, '#8ec0ff', '#1f63c9')}</defs>
          <path d="M2.6 7.2A5.4 5.4 0 0 1 12.2 4.3L13.4 3v4H9.4l1.4-1.4A3.5 3.5 0 0 0 4.6 7.2z" fill={`url(#${p}g)`} stroke="#123f86" strokeLinejoin="round" />
          <path d="M13.4 8.8A5.4 5.4 0 0 1 3.8 11.7L2.6 13V9h4l-1.4 1.4a3.5 3.5 0 0 0 6.2-1.6z" fill={`url(#${p}g)`} stroke="#123f86" strokeLinejoin="round" />
        </>
      );
    case 'file':
      // A page with its corner turned: a file to edit.
      return (
        <>
          <defs>{grad(`${p}g`, '#ffffff', '#dfe3ea')}</defs>
          <path d="M3.5 1.5h6l3 3v10h-9z" fill={`url(#${p}g)`} stroke="#5b6472" strokeLinejoin="round" />
          <path d="M9.5 1.5v3h3" fill="#c9cfd9" stroke="#5b6472" strokeLinejoin="round" />
          <path d="M5.5 7.5h5M5.5 9.5h5M5.5 11.5h3" stroke="#2a62c9" strokeWidth=".9" strokeLinecap="round" />
        </>
      );
    case 'download':
      return (
        <>
          <defs>{grad(`${p}g`, '#86d873', '#2b8a1d')}</defs>
          <path d="M6 1.5h4v5.4h2.8L8 12 3.2 6.9H6z" fill={`url(#${p}g)`} stroke="#1c5c12" strokeLinejoin="round" />
          <path d="M2 14.5h12" stroke="#7a7062" strokeWidth="1.6" strokeLinecap="round" />
        </>
      );
    case 'warn':
      return (
        <>
          <defs>{grad(`${p}g`, '#fff3a0', '#f0b400')}</defs>
          <path d="M8 1.5 15 14.2H1z" fill={`url(#${p}g)`} stroke="#8a6400" strokeLinejoin="round" />
          <path d="M8 6v4" stroke="#3a2a00" strokeWidth="1.6" strokeLinecap="round" />
          <circle cx="8" cy="12.1" r=".9" fill="#3a2a00" />
        </>
      );
  }
}

let seq = 0;

export function Icon({ name, size = 16, style }: { name: IconName; size?: number; style?: CSSProperties }) {
  // A per-instance prefix keeps gradient ids unique across the document.
  const p = `i${(seq = (seq + 1) % 1e6)}`;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      aria-hidden
      style={{ flex: 'none', display: 'block', overflow: 'visible', ...style }}
    >
      {draw(name, p)}
    </svg>
  );
}

/**
 * XP's flat toolbar button: nothing until the pointer is on it, then a raised
 * bevel, sunken while pressed. Large on purpose — 26px squares, the icon at
 * 16 — so the sidebar's controls are things to press, not glyphs to aim at.
 */
export function ToolButton({
  icon,
  label,
  title,
  onClick,
  danger,
  active,
  size = 26,
  disabled,
  spin,
  style
}: {
  icon: IconName;
  label?: ReactNode;
  title: string;
  onClick: (e: React.MouseEvent) => void;
  danger?: boolean;
  active?: boolean;
  size?: number;
  disabled?: boolean;
  /** Turns the icon while whatever it started is still running. */
  spin?: boolean;
  style?: CSSProperties;
}) {
  return (
    <button
      type="button"
      onClick={e => {
        e.stopPropagation();
        onClick(e);
      }}
      onMouseDown={e => e.stopPropagation()}
      title={title}
      aria-label={title.split('\n')[0]}
      disabled={disabled}
      className={`tool-btn${danger ? ' tool-btn-danger' : ''}${active ? ' tool-btn-on' : ''}`}
      style={{ height: size, minWidth: size, ...style }}
    >
      <Icon name={icon} size={size >= 24 ? 16 : 14} style={spin ? { animation: 'spin .9s linear infinite' } : undefined} />
      {label != null && <span className="tool-btn-label">{label}</span>}
    </button>
  );
}

/** The round fold button on an Explorer task-pane group's header. */
export function Chevron({ open }: { open: boolean }) {
  return (
    <span className="tp-chev" data-open={open ? 'yes' : 'no'} aria-hidden>
      <svg width="10" height="10" viewBox="0 0 10 10">
        <path d="M2 5.2 5 2.4l3 2.8M2 8.2 5 5.4l3 2.8" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    </span>
  );
}

/** A board's shape as a white line glyph, for the glass buttons on the title bar. */
export function LayoutGlyph({ n }: { n: 1 | 2 | 3 | 4 }) {
  const lines = { 1: '', 2: 'M9 3v11', 3: 'M9 3v11M9 8.5h7', 4: 'M9 3v11M2 8.5h14' }[n];
  return (
    <svg width="18" height="16" viewBox="0 0 18 16" aria-hidden style={{ display: 'block' }}>
      <rect x="1.5" y="1.5" width="15" height="13" rx="2" fill="rgba(255,255,255,.14)" stroke="currentColor" strokeWidth="1.3" />
      <path d="M1.5 4.2h15" stroke="currentColor" strokeWidth="2.2" />
      {lines && <path d={lines} stroke="currentColor" strokeWidth="1.2" fill="none" />}
    </svg>
  );
}

/** The sidebar toggle on the title bar: a window with its left pane, filled while pinned. */
export function SidebarGlyph({ pinned }: { pinned: boolean }) {
  return (
    <svg width="18" height="16" viewBox="0 0 18 16" aria-hidden style={{ display: 'block' }}>
      <rect x="1.5" y="1.5" width="15" height="13" rx="2" fill="none" stroke="currentColor" strokeWidth="1.3" />
      <rect x="2.2" y="2.2" width="4.8" height="11.6" rx="1" fill="currentColor" opacity={pinned ? 0.85 : 0.25} />
    </svg>
  );
}
