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
});
