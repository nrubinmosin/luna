import { useLayoutEffect, useRef } from 'react';
import type { BackgroundHelperDto } from '../../ipc/agents';
import { Icon } from '../../shared/ui/icons';
import { countHelpers, countWords, summaryNames } from './helperSummary';

/** What sets the pill's room: the title bar's flexible slot it sits in (Pane.tsx). */
const SLOT = '.pane-helpers';

/**
 * The inside of a chat's helper pill on its pane's title bar. The counts come
 * in three lengths, of which the pane's width shows one (panes.css): in words,
 * as symbols, or the total with the asking. After the words go the names of
 * those at work, asking ones first, as many whole names as the slot has room
 * for: the slot is the bar's flexible space, so the names take only what the
 * settings beside it leave over. Which fit is measured here, as the slot
 * resizes; no name is ever cut in half.
 */
export function HelperPill({ helpers, now }: { helpers: BackgroundHelperDto[]; now: number }) {
  const c = countHelpers(helpers);
  const names = summaryNames(helpers, now);
  const box = useRef<HTMLSpanElement>(null);
  const key = names.map(n => n.text).join('\n');

  useLayoutEffect(() => {
    const el = box.current;
    const pill = el?.parentElement;
    const slot = el?.closest<HTMLElement>(SLOT);
    if (!el || !pill || !slot) return;
    const fit = () => {
      const items = [...el.querySelectorAll<HTMLElement>('[data-name]')];
      el.style.display = '';
      for (const i of items) i.style.display = '';
      // Measured at its own width, not the slot's, which may be squeezed.
      pill.style.maxWidth = 'none';
      // The counts' own width is the least the slot may shrink to: the names
      // give way first, then the settings, never the counts.
      slot.style.minWidth = `${pill.offsetWidth - el.offsetWidth}px`;
      const room = slot.clientWidth;
      for (let n = items.length - 1; n >= 0 && pill.offsetWidth > room; n--) items[n]!.style.display = 'none';
      if (pill.offsetWidth > room || items.every(i => i.style.display === 'none')) el.style.display = 'none';
      pill.style.maxWidth = '';
    };
    fit();
    // The pill gone (no helpers left), the slot holds nothing to keep room for.
    const done = () => {
      slot.style.minWidth = '';
    };
    if (typeof ResizeObserver === 'undefined') return done;
    const ro = new ResizeObserver(fit);
    ro.observe(slot);
    return () => {
      ro.disconnect();
      done();
    };
  }, [key, c.working, c.idle, c.asks]);

  const ask = (n: number) => (
    <>
      <b className="ask-mark" aria-hidden />
      {n}
    </>
  );
  // Only helpers that are gone: said so, rather than an empty pill.
  const words = c.working + c.idle > 0 ? countWords({ ...c, asks: 0 }) : [` gone`];
  return (
    <>
      <Icon name="agents" size={12} />
      <span className="hp-words">
        {words.join(' · ')}
        {c.asks > 0 && (
          <>
            {words.length > 0 && ' · '}
            {ask(c.asks)} asks
          </>
        )}
      </span>
      <span className="hp-short">
        {c.working > 0 && <span title="working">▸{c.working}</span>}
        {c.idle > 0 && <span title="idle">◦{c.idle}</span>}
        {c.asks > 0 && <span>{ask(c.asks)}</span>}
      </span>
      <span className="hp-tiny">
        {c.working + c.idle}
        {c.asks > 0 && <span>{ask(c.asks)}</span>}
      </span>
      {/* Always there, empty or not: it is what the fitting measures by. */}
      <span ref={box} className="hp-names">
        {names.map(n => (
          <span key={n.id} data-name="" data-asking={n.asks ? 'yes' : 'no'}>
            {n.asks && <b className="ask-mark" aria-hidden />}
            {n.text}
          </span>
        ))}
      </span>
    </>
  );
}
