import { useEffect, useState } from 'react';
import {
  helpersAboveCaller,
  helperSettings,
  setHelperContextPct,
  setHelperMode,
  setHelpersAboveCaller,
  setLeadHandoff,
  setMaxHelpers,
  type HelperMode,
  type HelperSettingsDto
} from '../../../ipc/agents';
import { HANDOFF_AT } from '../../../shared/types';
import { ThresholdInput } from '../../chats/Handoff';
import { CheckRow, Group, Row } from '../SettingsRows';

/** The shares of a helper's context window its notices may say it is past. */
const CONTEXT_PCTS = [0, 30, 40, 50, 60, 70, 80, 90];

/** How helpers are held: whether one may run with more privilege than the
 *  session spawning it, how many places a chat's line of helpers has, the
 *  mode a helper takes when its spawn names none, past what share of its
 *  context its notices say so, and auto-handoff for leads (helpers with Luna
 *  tools) with its threshold. Kept by the Rust side, which
 *  holds every spawn to them (agents/privilege.rs `cap`, agents/spawn
 *  `max_descendants`, agents/handoff `lead.rs`). What turning the first on
 *  lets an agent do stays in view. */
export function HelpersSection() {
  const [on, setOn] = useState<boolean | null>(null);
  const [limits, setLimits] = useState<HelperSettingsDto | null>(null);
  useEffect(() => {
    void helpersAboveCaller().then(setOn);
    void helperSettings().then(setLimits);
  }, []);
  const places = limits ? Array.from({ length: limits.max - limits.min + 1 }, (_, i) => limits.min + i) : [];
  return (
    <Group
      title="Helpers"
      about={{
        summary: 'How helper permissions work',
        body: (
          <>
            <p>
              Off, a chat’s helpers do no more without asking than it does: a Plan-only chat cannot spawn one that
              edits, an Ask chat none that bypasses permissions, and the same for Codex’s sandbox and network. A chat
              approves its helpers’ permission prompts only for what it runs unasked itself. On, any chat with Luna
              tools may spawn helpers that bypass permissions or run with full access, and approve what they ask,
              whatever it runs in itself. Chats you make yourself are not affected either way.
            </p>
            <p>
              A helper reaches only the sessions under it: it cannot read, wait on or write to your chats or another
              line’s helpers, notify you or turn the PC off; your own chats with Luna tools can, when they ask to. Every
              session under one of your chats, helpers of helpers included, shares that chat’s places; a helper refused
              one hears which of its own it can delete, and your chat is told.
            </p>
          </>
        )
      }}
    >
      <CheckRow
        label="Allow helpers more permissions than their parent chat"
        desc="Off, a helper does no more without asking than the chat that spawned it."
        note="On, any chat with Luna tools may spawn helpers that bypass permissions or run with full access, and approve what they ask, whatever it runs in itself."
        warn
        checked={on ?? false}
        disabled={on === null}
        onChange={v => void setHelpersAboveCaller(v).then(setOn)}
      />
      <Row
        label="Helpers per chat"
        htmlFor="set-max-helpers"
        desc="The most sessions at work under one of your chats, its helpers’ helpers included."
        control={
          <select
            id="set-max-helpers"
            className="set-browser"
            value={limits?.maxHelpers ?? ''}
            disabled={!limits}
            onChange={e => void setMaxHelpers(Number(e.target.value)).then(setLimits)}
          >
            {places.map(n => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        }
      />
      <Row
        label="Helpers’ mode when not given"
        htmlFor="set-helper-mode"
        desc="The permission mode or sandbox a helper starts in when the chat spawning it names none."
        control={
          <select
            id="set-helper-mode"
            className="set-browser"
            value={limits?.helperMode ?? 'inherit'}
            disabled={!limits}
            onChange={e => void setHelperMode(e.target.value as HelperMode).then(setLimits)}
          >
            <option value="inherit">The spawning chat’s</option>
            <option value="account">The account’s default</option>
          </select>
        }
      />
      <Row
        label="Tell me when a helper’s context passes"
        htmlFor="set-helper-context"
        desc="Notices about a helper past this share of its context window say how full it is; past 85% one comes on its own. A spawn may set its own."
        control={
          <select
            id="set-helper-context"
            className="set-browser"
            value={limits?.helperContextPct ?? 50}
            disabled={!limits}
            onChange={e => void setHelperContextPct(Number(e.target.value)).then(setLimits)}
          >
            {CONTEXT_PCTS.map(n => (
              <option key={n} value={n}>
                {n === 0 ? 'Never' : `${n}%`}
              </option>
            ))}
          </select>
        }
      />
      <CheckRow
        label="Auto-handoff for leads"
        desc="A helper with Luna tools, past this share of its context, writes a handoff and goes on in a fresh conversation, its helpers staying with it."
        note="One that can write no file goes on from a handoff Luna writes from its records. Its parent hears of each handoff with its next notice."
        checked={limits?.leadHandoff.on ?? true}
        disabled={!limits}
        onChange={v => void setLeadHandoff(v, limits?.leadHandoff.threshold ?? 60).then(setLimits)}
        below={
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <ThresholdInput
              id="set-lead-handoff-at"
              value={Math.max(HANDOFF_AT.min, limits?.leadHandoff.threshold ?? 60)}
              disabled={!limits || !limits.leadHandoff.on}
              onCommit={v => void setLeadHandoff(true, v).then(setLimits)}
            />
            <span style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)' }}>% of a lead’s context</span>
          </div>
        }
      />
    </Group>
  );
}
