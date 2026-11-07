import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Chat } from '../../../shared/types';
import { useChats } from '../../chats/chats.store';
import { titleFromTyping } from './title';

// The chats store reaches the terminals; xterm cannot run in jsdom.
vi.mock('../terminals', () => ({ forget: vi.fn() }));

const chat: Chat = {
  id: 'typed',
  name: 'chat 12',
  status: 'resting',
  context: 0,
  account: 'work',
  group: 0,
  worktree: false,
  provider: 'claude',
  model: 'Opus',
  effort: 'high',
  perm: 'Bypass'
};

const name = () => useChats.getState().findChat('typed')?.name;

beforeEach(() => {
  useChats.getState().addChat('/repo', chat, false);
});

describe('terminal title: the first line typed names the chat', () => {
  it('takes the line once Enter is pressed, keystrokes arriving in pieces', () => {
    const take = titleFromTyping('typed');
    take('fix the ');
    take('updater');
    expect(name()).toBe('chat 12');
    take('\r');
    expect(name()).toBe('fix the updater');
  });

  it('leaves out what the terminal answers by itself: cursor reports, focus, mouse', () => {
    const take = titleFromTyping('typed');
    take('\x1b[1;1R\x1b[O');
    take('hello\x1b[<35;1;14M there\r');
    expect(name()).toBe('hello there');
  });

  it('does not take a slash command, and takes the next real line', () => {
    const take = titleFromTyping('typed');
    take('/model\r');
    expect(name()).toBe('chat 12');
    take('write the notes\r');
    expect(name()).toBe('write the notes');
  });

  it('names the chat once only', () => {
    const take = titleFromTyping('typed');
    take('first\r');
    take('second\r');
    expect(name()).toBe('first');
  });

  it('leaves a chat that already has a name of its own alone', () => {
    useChats.getState().setName('typed', 'picked by hand', 'user');
    titleFromTyping('typed')('typed line\r');
    expect(name()).toBe('picked by hand');
  });
});
