import { LibraryDialog } from './LibraryDialog';
import { useLibrary } from './library.store';
import './library.css';

/**
 * The user's own skills and instructions, kept in Luna's library and laid out
 * into the accounts each is meant for and for the sessions in folders named here. Raised from the Accounts
 * window, or from the status bar while a copy needs a look.
 */
export function LibraryWindow() {
  const open = useLibrary(s => s.open);
  return open ? <LibraryDialog onClose={() => useLibrary.getState().setOpen(false)} /> : null;
}
