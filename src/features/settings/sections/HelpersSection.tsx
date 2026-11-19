import { useEffect, useState } from 'react';
import { helperSettings, setHelperContextPct, setMaxHelpers, type HelperSettingsDto } from '../../../ipc/agents';
import { Group, Row } from '../SettingsRows';

/** The shares of a helper's context window its notices may say it is past. */
const CONTEXT_PCTS = [0, 30, 40, 50, 60, 70, 80, 90];

/** How many places a chat's line of helpers has, and past what share of its
 *  context a helper's notices say so. Kept by the Rust side, which holds every
 *  spawn to them (agents/spawn `max_descendants`). */
export function HelpersSection() {
  const [limits, setLimits] = useState<HelperSettingsDto | null>(null);
  useEffect(() => {
    void helperSettings().then(setLimits);
  }, []);
  const places = limits ? Array.from({ length: limits.max - limits.min + 1 }, (_, i) => limits.min + i) : [];
  return (
    <Group
      title="Helpers"
      about={{
        summary: 'What a helper may do',
        body: (
          <>
            <p>
              A helper runs in the mode the chat spawning it asks for, or, asked for none, in what its CLI’s own
              settings say for that account and folder. A chat answers its own helpers’ questions and permission
              prompts. What a mode or a sandbox lets through is the CLI’s own.
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
      <Row
        label="Helpers per chat"
        htmlFor="set-max-helpers"
        desc="The most sessions at work under one of your chats, its helpers’ helpers included."
        control={
          <select
            id="set-max-helpers"
            className="set-select"
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
        label="Tell me when a helper’s context passes"
        htmlFor="set-helper-context"
        desc="Notices about a helper past this share of its context window say how full it is; past 85% one comes on its own. A spawn may set its own."
        control={
          <select
            id="set-helper-context"
            className="set-select"
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
    </Group>
  );
}
