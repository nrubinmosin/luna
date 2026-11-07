import type { ReactNode } from 'react';

/** A dialog's group of settings under a small caps heading; `aside` sits at the heading's far end. */

export function Section({ label, aside, children }: { label: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <div style={{ marginBottom: 14 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
        <span
          style={{
            fontSize: 'var(--fs-1)',
            fontWeight: 700,
            textTransform: 'uppercase',
            letterSpacing: '.04em',
            color: 'var(--faint)'
          }}
        >
          {label}
        </span>
        <span style={{ flex: 1 }} />
        {aside}
      </div>
      {children}
    </div>
  );
}
