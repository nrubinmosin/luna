import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CLOSE_GRACE_MS, HoverPopover, OPEN_DELAY_MS } from './HoverPopover';

function mount(onOuterClick = vi.fn()) {
  render(
    <div onClick={onOuterClick}>
      <HoverPopover face="3 helpers" label="Background helpers">
        {close => (
          <>
            <button>Watch</button>
            <button onClick={close}>Done</button>
          </>
        )}
      </HoverPopover>
      <button>elsewhere</button>
    </div>
  );
  return { face: screen.getByRole('button', { name: 'Background helpers' }), onOuterClick };
}

const panel = () => screen.queryByRole('dialog');

afterEach(() => vi.useRealTimers());

describe('HoverPopover', () => {
  it('opens on keyboard focus, closes on Escape and gives focus back to its face', async () => {
    const user = userEvent.setup();
    const { face } = mount();
    await user.tab();
    expect(face).toHaveFocus();
    expect(face).toHaveAttribute('aria-expanded', 'true');
    await user.tab();
    expect(screen.getByRole('button', { name: 'Watch' })).toHaveFocus();
    expect(panel()).not.toBeNull();
    await user.keyboard('{Escape}');
    expect(panel()).toBeNull();
    expect(face).toHaveFocus();
  });

  it('closes once focus has left both the face and the panel', async () => {
    const user = userEvent.setup();
    mount();
    await user.tab();
    expect(panel()).not.toBeNull();
    // Through the face's panel and out to the button beside it.
    await user.tab();
    await user.tab();
    await user.tab();
    expect(screen.getByRole('button', { name: 'elsewhere' })).toHaveFocus();
    expect(panel()).toBeNull();
  });

  it('opens on a hover that rests, and closes after a grace once the pointer has left', () => {
    vi.useFakeTimers();
    const { face } = mount();
    const wait = (ms: number) =>
      act(() => {
        vi.advanceTimersByTime(ms);
      });
    // React makes its enter and leave of the pointer's over and out.
    fireEvent.pointerOver(face);
    expect(panel()).toBeNull();
    wait(OPEN_DELAY_MS);
    expect(panel()).not.toBeNull();
    fireEvent.pointerOut(face, { relatedTarget: document.body });
    wait(CLOSE_GRACE_MS - 50);
    // Reaching the panel within the grace keeps it.
    fireEvent.pointerOver(panel()!, { relatedTarget: document.body });
    wait(CLOSE_GRACE_MS);
    expect(panel()).not.toBeNull();
    fireEvent.pointerOut(panel()!, { relatedTarget: document.body });
    wait(CLOSE_GRACE_MS);
    expect(panel()).toBeNull();
  });
  it('stays open on a click until a second click, Escape or a press elsewhere', async () => {
    const user = userEvent.setup();
    const { face, onOuterClick } = mount();
    await user.click(face);
    expect(panel()).not.toBeNull();
    await user.click(face);
    expect(panel()).toBeNull();
    await user.click(face);
    fireEvent.mouseDown(screen.getByRole('button', { name: 'elsewhere' }));
    expect(panel()).toBeNull();
    expect(onOuterClick).not.toHaveBeenCalled();
  });

  it('keeps clicks in the panel from what holds the face, and lets the panel close itself', async () => {
    const user = userEvent.setup();
    const { face, onOuterClick } = mount();
    await user.click(face);
    await user.click(screen.getByRole('button', { name: 'Watch' }));
    expect(onOuterClick).not.toHaveBeenCalled();
    expect(panel()).not.toBeNull();
    await user.click(screen.getByRole('button', { name: 'Done' }));
    expect(panel()).toBeNull();
  });
});
