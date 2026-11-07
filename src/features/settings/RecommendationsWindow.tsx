import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { PROVIDER_LABEL, accountKey, type Account } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { ConfigEditor } from '../accounts/ConfigEditor';
import { RecommendedDialog } from '../accounts/RecommendedDialog';
import { recommendedCount, useRecommended } from '../accounts/recommended';
import { Recommendations } from './Recommendations';
import { useRecommendations } from './recommendations.store';
import { Section } from '../../shared/ui/Section';

/** One account's recommended settings: how many it has, what the everyday set still lacks, and the way in. */
function AccountRow({
  account,
  count,
  missing,
  onOpen
}: {
  account: Account;
  count: string;
  missing: number;
  onOpen: () => void;
}) {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 6,
        padding: '4px 0',
        borderBottom: '1px solid var(--line)',
        fontSize: 'var(--fs-3)'
      }}
    >
      <span title={PROVIDER_LABEL[account.provider]} style={{ flex: 'none', display: 'flex' }}>
        <Icon name={account.provider} />
      </span>
      <span
        style={{ fontWeight: 700, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
      >
        {account.name}
      </span>
      <span
        style={{
          flex: 1,
          fontSize: 'var(--fs-1)',
          color: 'var(--faint)',
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis'
        }}
      >
        {count}
        {missing > 0 && ` · ${missing} recommended not set`}
      </span>
      <button className="slim" onClick={onOpen}>
        Settings…
      </button>
    </div>
  );
}

function RecommendationsWindow({ onClose }: { onClose: () => void }) {
  const accounts = useAccounts(s => s.accounts);
  const [recommending, setRecommending] = useState<Account | null>(null);
  const [editing, setEditing] = useState<Account | null>(null);
  const inner = !!recommending || !!editing;
  const lists = useRecommended(accounts, inner);

  useEffect(() => {
    // Escape belongs to the dialog on top while there is one.
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && !inner && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [inner, onClose]);

  const host = useDialogHost();
  if (!host) return null;

  return createPortal(
    <div
      className="scrim"
      onClick={e => {
        e.stopPropagation();
        onClose();
      }}
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.35)',
        backdropFilter: 'blur(1px)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 70
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{
          width: 560,
          maxWidth: 'calc(100vw - 32px)',
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Recommendations</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body" style={{ maxHeight: 'calc(100vh - 120px)', overflowY: 'auto' }}>
          <Section label="What Luna relies on">
            <Recommendations />
          </Section>
          <Section label="Settings worth having, by account">
            {accounts.length === 0 && (
              <div style={{ fontSize: 'var(--fs-3)', color: 'var(--faint)' }}>No accounts yet.</div>
            )}
            {accounts.map(a => {
              const list = lists[accountKey(a.provider, a.name)];
              return (
                <AccountRow
                  key={accountKey(a.provider, a.name)}
                  account={a}
                  count={recommendedCount(list)}
                  missing={(list ?? []).filter(r => r.defaultOn && !r.applied).length}
                  onOpen={() => setRecommending(a)}
                />
              );
            })}
            <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 6 }}>
              What works well for each CLI inside Luna. These are yours to take or leave, so the status bar does not
              count them.
            </div>
          </Section>
        </div>
      </div>
      {recommending && (
        <RecommendedDialog
          account={recommending}
          onClose={() => setRecommending(null)}
          onEdit={() => {
            setEditing(recommending);
            setRecommending(null);
          }}
        />
      )}
      {editing && <ConfigEditor account={editing} onClose={() => setEditing(null)} />}
    </div>,
    host
  );
}

/** The one place for every recommendation, opened from its button in the status bar. */
export function RecommendationsDialog() {
  const open = useRecommendations(s => s.open);
  return open ? <RecommendationsWindow onClose={() => useRecommendations.getState().setOpen(false)} /> : null;
}
