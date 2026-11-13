// The function scripts/fit-check.mjs runs in the page it checks, on its own
// so that the linter gives it the browser's globals and the Node script none.

/** Takes `steps` on the page in order and returns what the last one gave.
 *  The only code of the extras that reaches the page: its source is fixed,
 *  and the steps come in as its argument, by value. It must stand alone,
 *  using nothing from this module: fit-check.mjs sends its source text, not
 *  the function. */
export async function pageSteps(steps) {
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const store = path => import(`/src/${path}`);
  const press = async el => {
    if (!el) throw new Error('nothing to press');
    el.scrollIntoView({ block: 'nearest' });
    await sleep(200);
    el.click();
  };
  const linkChooser = () => [...document.querySelectorAll('.window')].find(w => w.textContent.includes('Copy link'));
  const holds = (el, text) => !!el && (text == null || el.textContent.includes(text));
  const actions = {
    wait: sleep,
    click: selector => press(document.querySelector(selector)),
    clickText: (selector, text) =>
      press([...document.querySelectorAll(selector)].find(e => e.textContent.trim() === text)),
    // A click where the element stands, if it is there at all.
    clickIfThere: selector => document.querySelector(selector)?.click(),
    hover: selector => {
      const el = document.querySelector(selector);
      if (!el) throw new Error(`nothing at ${selector}`);
      el.dispatchEvent(new PointerEvent('pointerover', { bubbles: true, pointerType: 'mouse' }));
    },
    maxWidth: (selector, px) => {
      document.querySelector(selector).style.maxWidth = `${px}px`;
    },
    scrollToEnd: selector => {
      document.querySelector(selector).scrollTop = 1e6;
    },
    scrollBody: () => {
      const b = document.querySelector('.scrim > .window > .window-body');
      const was = b.scrollTop;
      b.scrollTop = was > 60 ? was - 60 : was + 60;
      return b.scrollTop !== was;
    },
    // The account box at the very bottom of the dialog's body, then opened:
    // the body's content pushed 320px down, so it scrolls there at any size.
    lowPicker: async () => {
      const body = document.querySelector('.scrim > .window > .window-body');
      const face = document.querySelector('.picker-face');
      body.firstElementChild.style.marginTop = '320px';
      await sleep(100);
      body.scrollTop += face.getBoundingClientRect().bottom - body.getBoundingClientRect().bottom + 6;
      await sleep(200);
      face.click();
    },
    link: async url =>
      (await store('features/browsers/browsers.store.ts')).useLinkChooser
        .getState()
        .show(url, 'demo-1', innerWidth - 120, innerHeight - 60),
    browsers: async count => {
      const s = await store('features/browsers/browsers.store.ts');
      const d = s.useBrowsers.getState().data;
      const found = Array.from({ length: count }, (_, i) => ({
        id: 'b' + i,
        name: 'Browser profile ' + i,
        command: 'x %1'
      }));
      s.useBrowsers.setState({ data: { ...d, found } });
    },
    // The first chat's helpers, each a copy of its first under one of `names`;
    // `done` leaves them idle with nothing left running.
    helpers: async (names, done) => {
      const s = (await store('features/agents/background.store.ts')).useBackground;
      const h = s.getState().helpers.find(x => x.parent === 'demo-1');
      const idle = done ? { state: 'idle', leftRunning: [] } : {};
      s.setState({ helpers: names.map((name, i) => ({ ...h, id: h.id + '-' + i, name, ...idle })) });
    },
    has: (selector, text) => holds(document.querySelector(selector), text),
    linkChooser: text => holds(linkChooser(), text)
  };
  let last;
  for (const [name, ...args] of steps) {
    if (!Object.hasOwn(actions, name)) throw new Error(`no page step called ${name}`);
    last = await actions[name](...args);
  }
  return last;
}
