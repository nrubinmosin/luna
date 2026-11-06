import { useEffect } from 'react';
import { ACCENT } from '../../shared/lib/format';
import { Icon } from '../../shared/ui/icons';
import { useAccountsWindow } from '../accounts/AccountsPanel';
import { useSettings } from './SettingsDialog';
import { openChecks, useRecommendations } from './recommendations.store';

const EVERY_MS = 10 * 60_000;

/**
 * The way to the recommendations, always there so they can be found, with a
 * count while things Luna relies on are off. Looks again now and then, when
 * the window comes back, and when a dialog that can change the answer closes.
 */
export function RecommendationsChip() {
  const checks = useRecommendations(s => s.checks);
  const settings = useSettings(s => s.open);
  const accounts = useAccountsWindow(s => s.open);
  const shown = useRecommendations(s => s.open);

  useEffect(() => {
    const look = () => void useRecommendations.getState().refresh();
    const t = setInterval(look, EVERY_MS);
    window.addEventListener('focus', look);
    return () => {
      clearInterval(t);
      window.removeEventListener('focus', look);
    };
  }, []);

  useEffect(() => {
    if (!settings && !accounts && !shown) void useRecommendations.getState().refresh();
  }, [settings, accounts, shown]);

  const open = openChecks(checks).length;
  return (
    <div
      onClick={() => useRecommendations.getState().setOpen(true)}
      title={
        open
          ? `Recommendations — ${open} thing${open === 1 ? '' : 's'} Luna relies on ${open === 1 ? 'is' : 'are'} off; click to see`
          : 'Recommendations — what Luna relies on, and settings worth having for each account'
      }
      className="sb-field click"
      style={{
        cursor: 'default', whiteSpace: 'nowrap', fontVariantNumeric: 'tabular-nums', fontWeight: 700,
        color: open ? ACCENT : undefined, padding: open ? undefined : '0 6px'
      }}
    >
      <Icon name="recommend" />
      {open > 0 && open}
    </div>
  );
}
