import { MAX_GROUPS, currentLayout, groupLabel, usePanes, type Layout } from '../features/panes/panes.store';
import { allChats, useChats } from '../features/chats/chats.store';
import { Icon, LayoutGlyph, SidebarGlyph, type IconName } from '../shared/ui/icons';
import { useShell, type ThemePref } from './shell.store';

const THEMES: Record<ThemePref, { icon: IconName; title: string }> = {
  system: { icon: 'auto', title: 'Theme: as the system' },
  light: { icon: 'sun', title: 'Theme: light' },
  dark: { icon: 'moon', title: 'Theme: dark' }
};

/**
 * The board's controls, on the window's title bar: the sidebar, the four
 * layouts, the groups, Even out and Clear, and the theme. They sat in a
 * card at the top of the sidebar, which cost its height and went away with
 * it; up here they are always within reach, whatever the sidebar is doing.
 */
export function BoardTools() {
  const layout = usePanes(currentLayout);
  const group = usePanes(s => s.group);
  const count = usePanes(s => s.groups.length);
  // Only a group nobody has a chat in can go; moving chats out is left to
  // the user rather than done behind their back.
  const occupied = useChats(s => allChats(s.folders).some(c => c.group === group));
  const pinned = useShell(s => s.pinned);
  const theme = useShell(s => s.theme);
  const { setLayout, setGroup, addGroup, resetGroup, resetSplits } = usePanes.getState();

  return (
    <div className="tb-tools">
      <button
        className="tb-btn"
        onClick={() => useShell.getState().setPinned(!pinned)}
        title={pinned ? 'Hide the sidebar — hovering the left edge brings it out' : 'Keep the sidebar open'}
        aria-pressed={pinned}
      >
        <SidebarGlyph pinned={pinned} />
      </button>
      <span className="tb-sep" />
      <div className="tb-seg" role="group" aria-label="Layout">
        {([1, 2, 3, 4] as Layout[]).map(n => (
          <button
            key={n}
            aria-pressed={layout === n}
            onClick={() => setLayout(n)}
            title={`${n} pane${n > 1 ? 's' : ''}`}
          >
            <LayoutGlyph n={n} />
          </button>
        ))}
      </div>
      {/* One level above the layouts: each group remembers its own boards, so
          a set of chats can be parked and brought back whole. */}
      <div className="tb-seg" role="group" aria-label="Group">
        {Array.from({ length: count }, (_, g) => (
          <button
            key={g}
            className={group === g && count > 1 ? 'tb-group-live' : undefined}
            aria-pressed={group === g}
            onClick={() => setGroup(g)}
            title={`Group ${groupLabel(g)}`}
          >
            {groupLabel(g)}
            {group === g && count > 1 && (
              <span
                role="button"
                className="tb-group-x"
                aria-disabled={occupied}
                title={
                  occupied
                    ? `Group ${groupLabel(g)} still has chats — move or delete them to remove it`
                    : `Remove group ${groupLabel(g)}`
                }
                onClick={e => {
                  e.stopPropagation();
                  useChats.getState().removeGroup(g);
                }}
              >
                ×
              </span>
            )}
          </button>
        ))}
        {count < MAX_GROUPS && (
          <button onClick={addGroup} title={`New group ${groupLabel(count)}`}>
            +
          </button>
        )}
      </div>
      <span className="tb-sep" />
      <button className="tb-btn" onClick={() => resetSplits()} title="Even out the splits of this board">
        <Icon name="even" />
        Even out
      </button>
      <button
        className="tb-btn"
        onClick={() => resetGroup(group)}
        title={`Clear group ${groupLabel(group)} — empties its panes, keeps every chat`}
      >
        <Icon name="reset" />
        Clear
      </button>
      <span data-tauri-drag-region style={{ flex: 1, alignSelf: 'stretch' }} />
      <button
        className="tb-btn"
        onClick={() => useShell.getState().cycleTheme()}
        title={`${THEMES[theme].title} — click for the next one`}
      >
        <Icon name={THEMES[theme].icon} />
      </button>
    </div>
  );
}
