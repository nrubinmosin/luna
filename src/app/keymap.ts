import { useEffect } from 'react';
import { usePanes, type Layout } from '../features/panes/panes.store';

export function useKeymap(opts: { openNewChat: () => void; closeModal: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (e.key === 'Escape') {
        opts.closeModal();
        // A maximised pane steps back to the grid on Escape. The key still
        // reaches the CLI underneath — swallowing it here would eat the
        // interrupt the person was probably also asking for.
        usePanes.getState().setFocus(null);
        return;
      }
      if (!mod) return;
      if (e.key.toLowerCase() === 'n') {
        e.preventDefault();
        opts.openNewChat();
      } else if (['1', '2', '3', '4'].includes(e.key)) {
        e.preventDefault();
        usePanes.getState().setLayout(Number(e.key) as Layout);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
