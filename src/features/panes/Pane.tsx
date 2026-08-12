import type { CSSProperties } from 'react';
import { STATUS } from '../../shared/ui/status';
import { ACCENT, limitColor, tail2, tint } from '../../shared/lib/format';
import { useChats } from '../chats/chats.store';
import { usePanes } from './panes.store';
import { Terminal } from './Terminal';

export function Pane({ index, area }: { index: number; area: string }) {
  const chatId = usePanes(s => s.panes[index]);
  const over = usePanes(s => s.over === index);
  const layout = usePanes(s => s.layout);
  const { setOver, dropChat, closePane } = usePanes.getState();
  const chat = useChats(s => s.findChat(chatId));
  const folder = useChats(s => (chatId ? s.folderOf(chatId) : null));

  const st = chat ? STATUS[chat.status] : null;
  const t = folder ? tail2(folder.path) : null;
  const ctx = chat ? Math.round((chat.context ?? 0) * 100) : 0;
  const tokens = chat?.contextTokens ?? null;
  const ctxTitle =
    tokens != null
      ? `Context: ${(tokens / 1000).toFixed(1)}k of 200k tokens (${ctx}%) — input + cache, last turn`
      : 'Context window usage — waiting for the first turn';

  const chip: CSSProperties = {
    fontSize: 11.5, color: 'var(--dim)', background: 'var(--chip)',
    padding: '2px 6px', borderRadius: 5, whiteSpace: 'nowrap'
  };

  return (
    <div
      onDragOver={e => {
        // File drags belong to the terminal's attach handler, not pane placement.
        if (Array.from(e.dataTransfer.types).includes('Files')) return;
        e.preventDefault();
        setOver(index);
      }}
      onDragLeave={() => usePanes.setState(s => (s.over === index ? { over: -1 } : s))}
      onDrop={e => {
        if (Array.from(e.dataTransfer.types).includes('Files')) return;
        e.preventDefault();
        const id = usePanes.getState().drag || e.dataTransfer.getData('text/plain');
        if (id) dropChat(index, id);
        else setOver(-1);
      }}
      style={{
        gridArea: area, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column',
        background: 'var(--bg)', border: `1px solid ${over ? ACCENT : 'var(--line)'}`,
        borderRadius: 11, overflow: 'hidden',
        boxShadow: over ? `0 0 0 3px ${tint(28, 'transparent')}` : '0 1px 2px oklch(.4 .04 160 / .07)'
      }}
    >
      {chat && folder ? (
        <>
          <div
            style={{
              flex: 'none', height: 38, display: 'flex', alignItems: 'center', gap: 8, padding: '0 10px',
              background: chat.status === 'working' ? tint(11, 'var(--panel)') : 'var(--panel)',
              borderBottom: '1px solid var(--line)'
            }}
          >
            <span style={{ width: 7, height: 7, borderRadius: '50%', flex: 'none', background: st!.color, animation: st!.anim }} />
            <span style={{ flex: '1 1 auto', minWidth: 60, fontSize: 13, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {chat.name}
            </span>
            <span style={{ fontSize: 11.5, color: 'var(--faint)', flex: 'none', whiteSpace: 'nowrap' }}>{st!.label}</span>
            <span
              onClick={() => closePane(index)}
              className="hover-bg"
              style={{ width: 18, height: 18, borderRadius: 5, display: 'grid', placeItems: 'center', fontSize: 12, color: 'var(--dim)', cursor: 'default', flex: 'none' }}
            >
              ✕
            </span>
          </div>
          <div style={{ flex: 'none', display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 5, padding: '7px 10px 8px', background: 'var(--panel)', borderBottom: '1px solid var(--line)' }}>
            <span style={{ fontSize: 11, color: 'var(--faint)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: '100%' }}>
              {t!.parent} / {t!.leaf}
            </span>
            <span
              title={`Running on Claude account "${chat.account}"`}
              style={{ ...chip, display: 'flex', alignItems: 'center', gap: 4, maxWidth: 160 }}
            >
              <span style={{ width: 5, height: 5, borderRadius: '50%', flex: 'none', background: 'var(--accent)' }} />
              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{chat.account}</span>
            </span>
            <span style={chip}>{chat.model}</span>
            <span style={chip}>effort: {chat.effort}</span>
            <span style={chip}>{chat.perm}</span>
            {chat.worktree && <span style={chip}>worktree</span>}
            <span
              title={ctxTitle}
              style={{ display: 'flex', alignItems: 'center', gap: 5, padding: '2px 6px 2px 5px', borderRadius: 5, background: 'var(--chip)', whiteSpace: 'nowrap' }}
            >
              <span style={{ width: 22, height: 4, borderRadius: 2, background: 'var(--hover)', overflow: 'hidden', flex: 'none' }}>
                <span style={{ display: 'block', height: '100%', borderRadius: 2, width: `${ctx}%`, background: limitColor(chat.context ?? 0) }} />
              </span>
              <span style={{ fontSize: 11.5, color: limitColor(chat.context ?? 0), fontVariantNumeric: 'tabular-nums' }}>{ctx}%</span>
              <span style={{ fontSize: 11.5, color: 'var(--faint)' }}>
                {tokens != null ? `${(tokens / 1000).toFixed(0)}k ctx` : 'ctx'}
              </span>
            </span>
          </div>
          <Terminal chat={chat} folderPath={folder.path} />
        </>
      ) : (
        <div style={{ flex: 1, display: 'grid', placeItems: 'center', gap: 6, alignContent: 'center', color: 'var(--faint)' }}>
          <div style={{ width: 40, height: 30, border: '1.5px dashed var(--line)', borderRadius: 6 }} />
          <div style={{ fontSize: 13, whiteSpace: 'nowrap' }}>Drag a chat here</div>
          <div style={{ fontSize: 11.5, opacity: 0.75, whiteSpace: 'nowrap' }}>
            pane {index + 1} of {layout}
          </div>
        </div>
      )}
    </div>
  );
}
