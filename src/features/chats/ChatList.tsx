import { useMemo } from 'react';
import { usePanes } from '../panes/panes.store';
import { foldersOfGroup, useChats } from './chats.store';
import { FolderSection } from './FolderSection';

export function ChatList() {
  const group = usePanes(s => s.group);
  const all = useChats(s => s.folders);
  // Each group is its own workspace: only its chats are listed, and a folder
  // with none of them stays out of the sidebar while remaining in the launch
  // list the new-chat dialog offers.
  const folders = useMemo(() => foldersOfGroup(all, group), [all, group]);

  return (
    <div style={{ flex: 1, overflowY: 'auto', padding: '2px 10px 14px', display: 'flex', flexDirection: 'column', gap: 10 }}>
      {folders.map(f => (
        <FolderSection key={f.id} folder={f} />
      ))}
      {folders.length === 0 && (
        <div style={{ padding: '24px 8px', textAlign: 'center', color: 'var(--faint)', fontSize: 'var(--fs-4)' }}>
          No chats yet.
          <br />
          Press <span style={{ fontWeight: 600 }}>New chat</span> to start.
        </div>
      )}
    </div>
  );
}
