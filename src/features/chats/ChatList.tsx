import { useChats } from './chats.store';
import { FolderSection } from './FolderSection';

export function ChatList() {
  const folders = useChats(s => s.folders);

  return (
    <div style={{ flex: 1, overflowY: 'auto', padding: '2px 10px 14px', display: 'flex', flexDirection: 'column', gap: 10 }}>
      {folders.map(f => (
        <FolderSection key={f.id} folder={f} />
      ))}
      {folders.length === 0 && (
        <div style={{ padding: '24px 8px', textAlign: 'center', color: 'var(--faint)', fontSize: 13 }}>
          No chats yet.
          <br />
          Press <span style={{ fontWeight: 600 }}>New chat</span> to start.
        </div>
      )}
    </div>
  );
}
