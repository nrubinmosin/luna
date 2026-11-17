import { useMemo } from 'react';
import { allChats, useChats } from '../chats/chats.store';
import { useBackground } from './background.store';
import { namer, type NameOf } from './ownership';

/** A session's name as the window shows it now: its chat row's, or its helper's. */
export function useNameOf(): NameOf {
  const folders = useChats(s => s.folders);
  const helpers = useBackground(s => s.helpers);
  return useMemo(() => namer(allChats(folders), helpers), [folders, helpers]);
}
