import { useRef } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { useCloseWhenMoved } from './popover';

function Probe({ open, onClose }: { open: boolean; onClose: () => void }) {
  const anchor = useRef<HTMLButtonElement>(null);
  useCloseWhenMoved(open, anchor, onClose);
  return (
    <>
      <div data-testid="scroller">
        <button ref={anchor}>face</button>
      </div>
      <div data-testid="popover" />
    </>
  );
}

/** Puts the anchor somewhere else on screen, as a scroll of what holds it would. */
const moveAnchor = () =>
  vi.spyOn(screen.getByRole('button'), 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 120, 80, 24));

describe('useCloseWhenMoved', () => {
  it('closes once a scroll moves the anchor', () => {
    const onClose = vi.fn();
    render(<Probe open onClose={onClose} />);
    fireEvent.scroll(screen.getByTestId('popover'));
    expect(onClose).not.toHaveBeenCalled();
    moveAnchor();
    fireEvent.scroll(screen.getByTestId('scroller'));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('closes once a resize moves the anchor', () => {
    const onClose = vi.fn();
    render(<Probe open onClose={onClose} />);
    moveAnchor();
    fireEvent(window, new Event('resize'));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('watches nothing while closed', () => {
    const onClose = vi.fn();
    render(<Probe open={false} onClose={onClose} />);
    moveAnchor();
    fireEvent.scroll(screen.getByTestId('scroller'));
    expect(onClose).not.toHaveBeenCalled();
  });
});
