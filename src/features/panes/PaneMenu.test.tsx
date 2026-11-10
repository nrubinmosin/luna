import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { PaneMenu } from './Pane';

function mount() {
  const onClose = vi.fn();
  const run = vi.fn();
  const { rerender } = render(
    <>
      <button>More</button>
      <button>elsewhere</button>
    </>
  );
  const anchor = screen.getByRole('button', { name: 'More' });
  rerender(
    <>
      <button>More</button>
      <button>elsewhere</button>
      <PaneMenu
        anchor={anchor}
        onClose={onClose}
        items={[
          { label: 'Rename', title: 'Rename chat', run },
          { label: 'Delete chat…', title: 'Asks first', run }
        ]}
      />
    </>
  );
  return { anchor, onClose, run };
}

describe('PaneMenu', () => {
  it('closes when focus leaves it', async () => {
    const user = userEvent.setup();
    const { onClose } = mount();
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
    await user.click(screen.getByRole('button', { name: 'elsewhere' }));
    expect(onClose).toHaveBeenCalled();
  });

  it('takes Escape only inside it, and gives focus back to its button then', () => {
    const { anchor, onClose } = mount();
    const elsewhere = screen.getByRole('button', { name: 'elsewhere' });
    // Focus elsewhere with the menu still mounted: onClose is a mock here.
    act(() => elsewhere.focus());
    onClose.mockClear();
    const reached = vi.fn((e: KeyboardEvent) => e.defaultPrevented);
    elsewhere.addEventListener('keydown', reached);
    fireEvent.keyDown(elsewhere, { key: 'Escape' });
    expect(reached.mock.results[0]!.value).toBe(false);
    expect(onClose).not.toHaveBeenCalled();
    expect(elsewhere).toHaveFocus();

    act(() => screen.getByRole('menuitem', { name: 'Rename' }).focus());
    fireEvent.keyDown(screen.getByRole('menuitem', { name: 'Rename' }), { key: 'Escape' });
    expect(onClose).toHaveBeenCalled();
    expect(anchor).toHaveFocus();
  });
});
