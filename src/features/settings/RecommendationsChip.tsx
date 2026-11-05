import { useEffect } from 'react';
import { ACCENT } from '../../shared/lib/format';
import { Icon } from '../../shared/ui/icons';
import { useAccountsWindow } from '../accounts/AccountsPanel';
import { useSettings } from './SettingsDialog';
import { openChecks, useRecommendations } from './recommendations.store';

const EVERY_MS = 10 * 60_000;

/**
 * The bar's word about recommendations, and only while there is one: how many
 * things Luna relies on are off. Looks again now and then, when the window
 * comes back, and when a dialog that can change the answer closes.
 */
export function RecommendationsChip() {
  const checks = useRecommendations(s => s.checks);
  const settings = useSettings(s => s.open);
  const accounts = useAccountsWindow(s => s.open);

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
    if (!settings && !accounts) void useRecommendations.getState().refresh();
  }, [settings, accounts]);

  const open = openChecks(checks).length;
  if (!open) return null;
  return (
    <div
      onClick={() => useSettings.getState().setOpen(true)}
      title={`Recommendations — ${open} thing${open === 1 ? '' : 's'} Luna relies on ${open === 1 ? 'is' : 'are'} off; click to see`}
      className="sb-field click"
      style={{ cursor: 'default', whiteSpace: 'nowrap', fontVariantNumeric: 'tabular-nums', color: ACCENT, fontWeight: 700 }}
    >
      <Icon name="recommend" />
      {open}
    </div>
  );
}
