import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { ChatStatus } from '../types';
import { STATUS } from './status';
import { StatusDot } from './StatusDot';

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
});
