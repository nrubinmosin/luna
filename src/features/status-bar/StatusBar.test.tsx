import { render, screen } from '@testing-library/react';
import { beforeAll, describe, expect, it } from 'vitest';
import { FOLDERS } from '../../dev/demo/fixtures';
import type { Chat, ChatStatus } from '../../shared/types';
import { useChats } from '../chats/chats.store';
import { StatusBar } from './StatusBar';

// jsdom has no ResizeObserver; the bar only needs it to exist.
beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

const seat = (statuses: ChatStatus[]) => {
  const base = FOLDERS[0]?.chats[0] as Chat;
  const chats = statuses.map((status, i) => ({ ...base, id: `c${i}`, name: `Chat ${i}`, status }));
  useChats.setState({ folders: [{ id: 'f', path: 'C:\\p', open: true, picked: true, chats }] });
  render(
    <div data-app>
      <StatusBar />
    </div>
  );
};

describe('StatusBar', () => {
  it('counts working, waiting and background chats apart', () => {
    seat(['working', 'waiting', 'waiting', 'background', 'background', 'background', 'resting']);
    expect(screen.getByText('1 working')).toBeInTheDocument();
    expect(screen.getByText('2 waiting')).toBeInTheDocument();
    expect(screen.getByText('3 background')).toBeInTheDocument();
  });

  it('names the chats at work in the background', () => {
    seat(['working', 'background', 'background']);
    expect(screen.getByText('2 background')).toHaveAttribute('title', 'At work in the background:\nChat 1\nChat 2');
  });

  it('says so when none is', () => {
    seat(['working', 'resting']);
    expect(screen.getByText('0 background')).toHaveAttribute('title', 'Nothing is at work in the background');
  });
});
