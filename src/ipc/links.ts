import { call } from './core';

/** A link from a terminal, in the system's handler for it (links.rs `open_link`). */
export const openLink = (url: string) => call<void>('open_link', { url });
