import { useEffect, type KeyboardEvent as ReactKeyboardEvent, type RefObject } from 'react';

/** What Tab can reach inside `root`, in document order: enabled controls not
 *  taken out of the order, not hidden or inert, and not folded away in a
 *  closed <details> (its summary still counts). */
export function focusables(root: Element): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>('button, input, select, textarea, summary, [href], [tabindex]')].filter(
    el => {
      if (el.getAttribute('tabindex') === '-1' || (el as HTMLButtonElement).disabled) return false;
      if (el.closest('[hidden], [inert]')) return false;
      const folded = el.closest('details:not([open])');
      return !folded || el.parentElement === folded;
    }
  );
}

/**
 * Makes the dialog in `box` modal, the WAI-ARIA way, for as long as it is
 * mounted: every other child of `host` (the app, and a dialog this one was
 * opened over) goes inert, the focus moves in (to `initial` inside it, else
 * the dialog itself), and on close the inert goes and the focus returns to
 * whatever had it before. Dialogs stack: one opened over another suspends it
 * and gives it back. The returned handler, on the dialog's onKeyDown, keeps
 * Tab and Shift+Tab going round inside it.
 */
export function useModal(box: RefObject<HTMLElement | null>, host: Element | null, initial?: string) {
  useEffect(() => {
    const dialog = box.current;
    const own = host && dialog ? [...host.children].find(c => c.contains(dialog)) : undefined;
    if (!host || !dialog || !own) return;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // Only what is not inert already: what a dialog underneath made inert is
    // that dialog's to give back.
    const others = [...host.children].filter(c => c !== own && !c.hasAttribute('inert'));
    others.forEach(c => c.setAttribute('inert', ''));
    ((initial && dialog.querySelector<HTMLElement>(initial)) || dialog).focus();
    return () => {
      others.forEach(c => c.removeAttribute('inert'));
      if (opener?.isConnected) opener.focus();
    };
  }, [box, host, initial]);

  return (e: ReactKeyboardEvent) => {
    const dialog = box.current;
    // A key from a popup the dialog owns but portals elsewhere is the popup's.
    if (e.key !== 'Tab' || !dialog?.contains(e.target as Node)) return;
    const all = focusables(dialog);
    const first = all[0];
    const last = all[all.length - 1];
    if (!first || !last) return;
    const at = document.activeElement;
    // From the dialog itself (where it opens) Tab goes to the first control.
    if (e.shiftKey ? at === first || at === dialog : at === last || at === dialog) {
      e.preventDefault();
      (e.shiftKey ? last : first).focus();
    }
  };
}
