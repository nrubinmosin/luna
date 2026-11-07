import { useEffect, useState } from 'react';

/**
 * Where a dialog portals to: inside the [data-app] element, so the theme and
 * xp.css overrides reach it, but out of any draggable subtree. Looked up after
 * mount — a dialog that is open on the very first render would otherwise run
 * before [data-app] is in the document and land on <body>, unstyled.
 */
export function useDialogHost(): Element | null {
  const [host, setHost] = useState<Element | null>(() => document.querySelector('[data-app]'));
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- the app's root exists only once the first render has committed
    if (!host) setHost(document.querySelector('[data-app]') ?? document.body);
  }, [host]);
  return host;
}
