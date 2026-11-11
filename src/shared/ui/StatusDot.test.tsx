import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { STATUS } from './status';
import { StatusDot } from './StatusDot';

describe('StatusDot', () => {
  it('wears background in a hue and a pulse of its own', () => {
    render(<StatusDot status="background" />);
    const { animation } = screen.getByTitle('background').style;
    expect(animation).toContain('pulse');
    expect(animation).toContain('3.6s');
    expect(STATUS.background.color).not.toBe(STATUS.working.color);
    expect(STATUS.background.color).not.toBe(STATUS.waiting.color);
    expect(STATUS.background.anim).not.toBe(STATUS.working.anim);
  });
});
