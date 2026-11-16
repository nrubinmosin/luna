import { describe, expect, it } from 'vitest';
import type { BackgroundHelperDto } from '../../ipc/agents';
import { managedLine, namer, ownershipLine, suggestedChats } from './ownership';

const row = (id: string, over: Partial<BackgroundHelperDto> = {}): BackgroundHelperDto => ({
  id,
  parent: 'chat-a',
  name: id,
  provider: 'claude',
  state: 'idle',
  account: 'work',
  model: null,
  effort: 'low',
  worktree: false,
  startedMs: 0,
  turnStartedMs: null,
  lastActivityMs: null,
  task: null,
  ...over
});

const now = 10 * 60_000;
const lead = row('lead-1', { name: 'Lead', top: 'chat-a' });
const nameOf = namer(
  [
    { id: 'chat-a', name: 'Release' },
    { id: 'chat-b', name: 'Docs' }
  ],
  [lead]
);

describe('whose a helper is, as its row says', () => {
  it('names the chats and helpers as the window shows them, and an unknown one by its id', () => {
    expect(nameOf('chat-b')).toBe('Docs');
    expect(nameOf('lead-1')).toBe('Lead');
    expect(nameOf('gone-1')).toBe('gone-1');
  });

  it('says who manages it and which chat its line hangs from, once when they are the same', () => {
    expect(managedLine(row('h', { top: 'chat-a' }), nameOf)).toBe('managed by chat Release');
    expect(managedLine(row('h', { parent: 'lead-1', top: 'chat-a' }), nameOf)).toBe('managed by Lead · chat Release');
    expect(managedLine(row('h', { parent: null, top: null }), nameOf)).toBeNull();
    // Its chat deleted and the helper above it left alone: no chat, never the helper called one.
    expect(managedLine(row('h', { parent: 'lead-1', top: null }), nameOf)).toBe('managed by Lead · no chat above it');
  });

  it('adds the chats watching it and the person’s last move, from the row’s fields, the helper’s own chat last', () => {
    const h = row('h', {
      top: 'chat-a',
      watchedBy: ['chat-b'],
      moved: { from: 'chat-b', to: 'chat-a', atMs: now - 5 * 60_000, told: true }
    });
    expect(ownershipLine(h, nameOf, now)).toBe('watched by Docs · moved from Docs, 5m ago · managed by chat Release');
    const loose = row('h', {
      parent: 'chat-a',
      top: 'chat-a',
      moved: { from: null, to: 'chat-a', atMs: now, told: false }
    });
    expect(ownershipLine(loose, nameOf, now)).toBe('moved from no chat, 0s ago · managed by chat Release');
    expect(ownershipLine(row('h', { parent: null }), nameOf, now)).toBe('');
    const kid = row('h', { parent: 'lead-1', top: 'chat-a', watchedBy: ['chat-b'] });
    expect(ownershipLine(kid, nameOf, now)).toBe('managed by Lead · chat Release · watched by Docs');
    const left = row('h', { parent: 'lead-1', watchedBy: ['chat-b'] });
    expect(ownershipLine(left, nameOf, now)).toBe('managed by Lead · no chat above it · watched by Docs');
  });

  it('offers the chats refused a write into it, the latest first, each once', () => {
    const h = row('h', {
      suggested: [
        { chat: 'chat-b', atMs: 1 },
        { chat: 'chat-c', atMs: 2 }
      ]
    });
    expect(suggestedChats(h)).toEqual(['chat-c', 'chat-b']);
    expect(suggestedChats(row('h'))).toEqual([]);
    // B asked, then C, then B again: the core moved B's time on in place.
    const again = row('h', {
      suggested: [
        { chat: 'chat-b', atMs: 3 },
        { chat: 'chat-c', atMs: 2 }
      ]
    });
    expect(suggestedChats(again)).toEqual(['chat-b', 'chat-c']);
  });
});
