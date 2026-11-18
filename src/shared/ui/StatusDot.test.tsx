import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { ChatStatus } from '../types';
import { STATUS } from './status';
import { StatusDot } from './StatusDot';
import baseCss from '../../styles/base.css?raw';

describe('StatusDot', () => {
  it('tells every state apart by its color alone, and never moves', () => {
    const states = Object.keys(STATUS) as ChatStatus[];
    expect(new Set(states.map(s => STATUS[s].color)).size).toBe(states.length);
    for (const s of states) {
      const { unmount } = render(<StatusDot status={s} />);
      const { animation, transition } = screen.getByTitle(STATUS[s].label).style;
      expect(animation).toBe('');
      expect(transition).toBe('');
      unmount();
    }
  });

  it('tells working from background by shape too: a dot and a ring', () => {
    const { unmount } = render(<StatusDot status="working" />);
    // (jsdom drops the colors, oklch(), so the shape is read off its mark.)
    const working = screen.getByTitle('working');
    expect(working).toHaveAttribute('data-shape', 'dot');
    unmount();
    render(<StatusDot status="background" />);
    const background = screen.getByTitle('background');
    expect(background).toHaveAttribute('data-shape', 'ring');
  });

  it('keeps every dot in forced colors, which drop shadows and paint backgrounds over', () => {
    // jsdom matches no forced-colors media, so the rule is read as written:
    // each state gets a system color, and the ring a border of one.
    const rules = baseCss.slice(baseCss.indexOf('@media (forced-colors: active)'));
    expect(rules).toMatch(/\.status-dot \{\s*background: CanvasText !important;/);
    expect(rules).toMatch(
      /\.status-dot\[data-shape="ring"\] \{\s*background: transparent !important;\s*border-color: CanvasText/
    );
    for (const s of Object.keys(STATUS) as ChatStatus[]) {
      const { unmount } = render(<StatusDot status={s} />);
      const dot = screen.getByTitle(STATUS[s].label);
      expect(dot).toHaveClass('status-dot');
      expect(dot).toHaveAttribute('data-status', s);
      unmount();
    }
  });
});
