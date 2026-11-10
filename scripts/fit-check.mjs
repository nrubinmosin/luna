// Checks that the UI fits small screens: every demo scene of src/dev/demo/ and
// the popups a click opens in them, at the sizes a laptop gives the window,
// with every control of every dialog, popover and menu within reach. Run by
// scripts/fit-check.ps1, in the screenshots image, against `vite` serving the
// frontend on :1420; it drives headless Chromium over the DevTools protocol.
//
// A control is within reach when it is on screen, or inside a scroller that is
// itself within reach: the person can scroll to it. A problem is
//
// - a control of a dialog, popover, menu or the board out of reach;
// - a control of a dialog or popup standing past its side edge, or a button
//   whose label breaks onto a second line (a row too narrow for its buttons);
// - a popover, menu or drop-down cut off where it stands, by the window or by
//   anything round it that clips (a dialog's scrolling body), so its options
//   show only after scrolling something else;
// - a popover still open after a scroll moved its button away (`closes`).
//
// Each scene and size is shot to <out>/<scene>__<size>.png, the findings go to
// <out>/report.json, and any problem exits 1. Scenes ending in `-dark` are left
// out: the theme changes colors, not sizes.
//
//   node scripts/fit-check.mjs <out> [scene,...] [size,...]

import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';

const BASE = 'http://localhost:1420/';

/** The window sizes, in CSS pixels at their display scaling: 1280x720 and
 *  1024x600 screens, the first at 125% and 150%, and 1024x600 at 150%. */
const SIZES = [
  { name: '1280x720', w: 1280, h: 720, s: 1 },
  { name: '1024x600', w: 1024, h: 600, s: 1 },
  { name: '1280x720@125', w: 1024, h: 576, s: 1.25 },
  { name: '1280x720@150', w: 853, h: 480, s: 1.5 },
  { name: '1024x600@150', w: 683, h: 400, s: 1.5 }
];

// Page-side snippets for the extras. A control is scrolled into view before it
// is clicked, as a person would have to.
const store = path => `(await import('/src/${path}'))`;
const wait = ms => `await new Promise(r => setTimeout(r, ${ms}));`;
const press = el =>
  `{ const el = ${el}; if (!el) throw new Error('nothing to press'); el.scrollIntoView({ block: 'nearest' }); ${wait(200)} el.click(); }`;
const click = selector => press(`document.querySelector(${JSON.stringify(selector)})`);
const clickText = (selector, text) =>
  press(
    `[...document.querySelectorAll(${JSON.stringify(selector)})].find(e => e.textContent.trim() === ${JSON.stringify(text)})`
  );
/** Scrolls the open dialog's body by 60px, whichever way it can go, and says
 *  whether it moved: a body that fits has nothing to scroll. */
const scrollBody = `const b = document.querySelector('.scrim > .window > .window-body'); const was = b.scrollTop; b.scrollTop = was > 60 ? was - 60 : was + 60; return b.scrollTop !== was;`;
const has = selector => `document.querySelector(${JSON.stringify(selector)})`;
/** The first chat with fourteen helpers at work, more than any window is tall. */
const manyHelpers =
  `{ const s = ${store('features/agents/background.store.ts')}.useBackground;` +
  `const h = s.getState().helpers.find(x => x.parent === 'demo-1');` +
  `s.setState({ helpers: Array.from({ length: 14 }, (_, i) => ({ ...h, id: h.id + '-' + i, name: 'helper ' + i })) }); } ${wait(300)}`;
/** Rests the pointer on the first pane's helper pill, which opens its list.
 *  A hover, not focus: a headless page that is not focused fires no focus events. */
const hoverPill = `{ const p = document.querySelector('.pane .hpill'); if (!p) throw new Error('no pill'); p.dispatchEvent(new PointerEvent('pointerover', { bubbles: true, pointerType: 'mouse' })); } ${wait(500)}`;
/** Holds the first pane to `px` wide, whatever the board gives it. */
const narrowPane = px => `document.querySelector('.pane').style.maxWidth = '${px}px'; ${wait(200)}`;
const linkChooser = `[...document.querySelectorAll('.window')].find(w => w.textContent.includes('Copy link'))`;

/** What only a click (or a store a click would set) opens: a scene to start
 *  from, what to do in it, and what must then be open (`opens`, a page
 *  expression), or the check would measure the scene beneath and pass.
 *  `then` runs once it is open; with `closes`, no popup may be left open
 *  after it if it says it scrolled something. */
const EXTRAS = [
  { name: 'newchat-picker', scene: 'newchat', setup: click('.picker-face'), opens: has('.picker-list') },
  {
    // The account box at the very bottom of the dialog's body, then opened:
    // the body's content pushed 320px down, so it scrolls there at any size.
    name: 'newchat-picker-low',
    scene: 'newchat',
    setup:
      `const body = document.querySelector('.scrim > .window > .window-body'); const face = document.querySelector('.picker-face');` +
      `body.firstElementChild.style.marginTop = '320px'; ${wait(100)}` +
      `body.scrollTop += face.getBoundingClientRect().bottom - body.getBoundingClientRect().bottom + 6; ${wait(200)}` +
      `face.click();`,
    opens: has('.picker-list')
  },
  {
    name: 'newchat-picker-scroll',
    scene: 'newchat',
    setup: click('.picker-face'),
    opens: has('.picker-list'),
    then: scrollBody,
    closes: true
  },
  {
    name: 'settings-links-menu',
    scene: 'settings-links',
    setup: click('[aria-haspopup="menu"]'),
    opens: has('[role=menu]')
  },
  {
    name: 'library-editor',
    scene: 'library',
    setup: click('button[title^="Edit "][title$="SKILL.md"]'),
    opens: has('.config-text')
  },
  {
    name: 'library-addfolder',
    scene: 'library',
    setup: clickText('button', 'Add a folder…'),
    opens: has('[role=menu]')
  },
  { name: 'library-reach', scene: 'library', setup: click('.picker-face'), opens: has('.popover') },
  {
    name: 'library-reach-scroll',
    scene: 'library',
    setup: click('.picker-face'),
    opens: has('.popover'),
    then: scrollBody,
    closes: true
  },
  {
    name: 'handoff-previous',
    scene: 'handoff',
    setup:
      `document.querySelector('.scrim')?.click(); ${wait(300)}` + clickText('.handoff-link', 'previous conversation'),
    opens: has('.helper-view')
  },
  { name: 'color', scene: 'main', setup: click('[data-glyph="swatch"]'), opens: has('.popover .swatch') },
  {
    name: 'link',
    scene: 'main',
    setup: `${store('features/browsers/browsers.store.ts')}.useLinkChooser.getState().show('https://example.com/a/rather/long/link?with=query', 'demo-1', innerWidth - 120, innerHeight - 60);`,
    opens: linkChooser
  },
  {
    // More browsers than any window is tall.
    name: 'link-many',
    scene: 'main',
    setup:
      `const s = ${store('features/browsers/browsers.store.ts')};` +
      `s.useLinkChooser.getState().show('https://example.com/a/rather/long/link?with=query', 'demo-1', innerWidth - 120, innerHeight - 60); ${wait(400)}` +
      `const d = s.useBrowsers.getState().data;` +
      `s.useBrowsers.setState({ data: { ...d, found: Array.from({ length: 24 }, (_, i) => ({ id: 'b' + i, name: 'Browser profile ' + i, command: 'x %1' })) } });`,
    opens: `${linkChooser}?.textContent.includes('Browser profile 23')`
  },
  {
    // Three helpers done: every button of each row enabled, Open as chat too.
    name: 'helpers-done',
    scene: 'main',
    setup:
      `const s = ${store('features/agents/background.store.ts')}.useBackground;` +
      `const h = s.getState().helpers.find(x => x.parent === 'demo-1');` +
      `s.setState({ helpers: ['reviewer-handoff', 'reviewer-0.12', 'reviewer-library'].map((name, i) => ({ ...h, id: h.id + '-' + i, name, state: 'idle', leftRunning: [] })) }); ${wait(300)}` +
      click('.chat-row[data-active="yes"] .kids'),
    opens: `${has('.popover')}?.textContent.includes('reviewer-library')`
  },
  {
    // A chat's helpers, more than any window is tall.
    name: 'helpers-many',
    scene: 'main',
    setup:
      `const s = ${store('features/agents/background.store.ts')}.useBackground;` +
      `const h = s.getState().helpers.find(x => x.parent === 'demo-1');` +
      `s.setState({ helpers: Array.from({ length: 14 }, (_, i) => ({ ...h, id: h.id + '-' + i, name: 'helper ' + i })) }); ${wait(300)}` +
      click('.chat-row[data-active="yes"] .kids'),
    opens: `${has('.popover')}?.textContent.includes('helper 13')`
  },
  {
    // The same fourteen from the pane's pill, opened by a hover.
    name: 'helpers-pane',
    scene: 'main',
    setup: manyHelpers + hoverPill,
    opens: `${has('.helper-panel')}?.textContent.includes('helper 13')`
  },
  {
    // A pane narrow enough to fold its window buttons, and its "…" menu.
    name: 'pane-narrow-menu',
    scene: 'main',
    setup: narrowPane(380) + click('.pane .pane-more'),
    opens: has('.pane-menu')
  },
  {
    // The narrowest pill, on a pane at its least, and its list.
    name: 'pane-narrow-helpers',
    scene: 'main',
    setup: narrowPane(260) + manyHelpers + hoverPill,
    opens: `${has('.helper-panel')}?.textContent.includes('helper 13')`
  }
];

// The page side: for every dialog, popover and menu on screen and for the
// board, the controls out of reach, and how much of each popup is cut off.
const MEASURE = `(() => {
  const vw = innerWidth, vh = innerHeight;
  const scrollerOf = el => {
    for (let p = el.parentElement; p && p !== document.documentElement; p = p.parentElement) {
      const o = getComputedStyle(p).overflowY;
      if (o === 'auto' || o === 'scroll') return p;
    }
    return null;
  };
  const onScreen = r => r.top >= -0.5 && r.bottom <= vh + 0.5 && r.left >= -0.5 && r.right <= vw + 0.5;
  const reachable = el => { const s = scrollerOf(el); return s ? reachable(s) : onScreen(el.getBoundingClientRect()); };
  const shown = el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== 'hidden' && !el.closest('[hidden]'); };
  const label = el => (el.getAttribute('aria-label') || el.textContent.trim() || el.getAttribute('title') || el.tagName).trim().replace(/\\s+/g, ' ').slice(0, 32);
  // How many pixels of a box do not show as things stand: cut by the window
  // and by every ancestor that clips, with nothing scrolled.
  const cut = el => {
    const r = el.getBoundingClientRect();
    let t = Math.max(r.top, 0), b = Math.min(r.bottom, vh), l = Math.max(r.left, 0), rt = Math.min(r.right, vw);
    for (let p = el.parentElement; p && p !== document.documentElement; p = p.parentElement) {
      const s = getComputedStyle(p);
      if (s.overflowX === 'visible' && s.overflowY === 'visible') continue;
      const q = p.getBoundingClientRect();
      t = Math.max(t, q.top); b = Math.min(b, q.bottom); l = Math.max(l, q.left); rt = Math.min(rt, q.right);
    }
    return Math.round(r.height - Math.max(0, b - t) + (r.width - Math.max(0, rt - l)));
  };
  // A control past its box's side edge: a row too narrow for it. Within a part
  // of the box made to scroll sideways (the library's wide grid) it is only
  // scrolled out of view; a popup scrolling sideways is no such part.
  const pastSide = (c, box) => {
    const r = c.getBoundingClientRect(), q = box.getBoundingClientRect();
    if (r.right <= q.right + 1 && r.left >= q.left - 1) return false;
    for (let p = c.parentElement; p && p !== box; p = p.parentElement) {
      const o = getComputedStyle(p).overflowX;
      if (o === 'auto' || o === 'scroll') return false;
    }
    return true;
  };
  // How many lines a button's label takes: the distinct tops of its text.
  const lines = el => {
    const tops = new Set();
    const walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let t = walk.nextNode(); t; t = walk.nextNode()) {
      if (!t.textContent.trim()) continue;
      const range = document.createRange();
      range.selectNodeContents(t);
      for (const r of range.getClientRects()) if (r.width > 0) tops.add(Math.round(r.top / 4));
    }
    return tops.size;
  };
  // Checkboxes and radios are drawn on their labels; xp.css hides the inputs.
  const CONTROLS ='button, select, textarea, input:not([type=checkbox]):not([type=radio]), input + label, [role=tab], [role=option], [role=menuitem]';
  const POPUPS = '.popover, .picker-list, [role=menu], [data-app] > div > .window';
  const boxes = new Set([...document.querySelectorAll('.scrim > .window, ' + POPUPS)]);
  const rows = [];
  for (const box of boxes) {
    if (!shown(box)) continue;
    const dialog = !!box.closest('.scrim') && box.classList.contains('window');
    const controls = [...box.querySelectorAll(CONTROLS)].filter(shown);
    rows.push({
      kind: dialog ? 'dialog' : 'popup',
      title: (box.querySelector('.title-bar-text')?.textContent || box.getAttribute('aria-label') || box.className).trim().slice(0, 40),
      controls: controls.length,
      lost: controls.filter(c => !reachable(c)).map(label),
      past: controls.filter(c => pastSide(c, box)).map(label),
      wrapped: controls.filter(c => c.tagName === 'BUTTON' && lines(c) > 1).map(label),
      cut: dialog ? 0 : cut(box)
    });
  }
  const board = [...document.querySelectorAll('[data-app] ' + CONTROLS.split(', ').join(', [data-app] '))]
    .filter(c => !c.closest('.scrim, ' + POPUPS) && shown(c));
  rows.push({ kind: 'board', title: '', controls: board.length, lost: board.filter(c => !reachable(c)).map(label), past: [], wrapped: [], cut: 0 });
  return JSON.stringify({ vw, vh, dpr: devicePixelRatio, rows });
})()`;

/** The problems in one measurement, as lines. */
function problems(measured, closes) {
  const found = [];
  for (const r of measured.rows) {
    const what = `${r.kind}${r.title ? ` "${r.title}"` : ''}`;
    if (r.lost.length)
      found.push(
        `${what}: ${r.lost.length} of ${r.controls} controls out of reach (${r.lost.slice(0, 4).join(' | ')})`
      );
    if (r.past.length)
      found.push(`${what}: ${r.past.length} controls past its side edge (${r.past.slice(0, 4).join(' | ')})`);
    if (r.wrapped.length)
      found.push(
        `${what}: ${r.wrapped.length} button labels on two lines or more (${r.wrapped.slice(0, 4).join(' | ')})`
      );
    if (r.cut > 1) found.push(`${what}: ${r.cut}px cut off where it stands`);
    if (closes && r.kind === 'popup') found.push(`${what}: still open after its button scrolled away`);
  }
  return found;
}

/** How long one DevTools request may take, and the whole run. */
const REQUEST_MS = 30_000;
const RUN_MS = 40 * 60_000;

/** A DevTools session on the first page of the Chromium started below. Every
 *  request has a deadline, and the socket closing fails whatever waits on it:
 *  a stalled page fails the run rather than hold it. */
async function devtools() {
  let page;
  for (let i = 0; i < 150 && !page; i++) {
    try {
      const list = await (await fetch('http://127.0.0.1:9222/json/list')).json();
      page = list.find(t => t.type === 'page');
    } catch {
      // Not listening yet.
    }
    if (!page) await new Promise(r => setTimeout(r, 200));
  }
  if (!page) throw new Error('Chromium never offered a page to drive');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = () => reject(new Error('could not connect to Chromium'));
  });
  let seq = 0;
  const pending = new Map();
  let onLoad = null;
  const settle = (id, error, result) => {
    const waiting = pending.get(id);
    if (!waiting) return;
    pending.delete(id);
    clearTimeout(waiting.timer);
    if (error) waiting.reject(error);
    else waiting.resolve(result);
  };
  ws.onmessage = ev => {
    const m = JSON.parse(String(ev.data));
    if (m.id) settle(m.id, m.error && new Error(m.error.message), m.result);
    else if (m.method === 'Page.loadEventFired') onLoad?.();
  };
  ws.onclose = () => {
    for (const id of [...pending.keys()]) settle(id, new Error('Chromium went away'));
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      if (ws.readyState !== WebSocket.OPEN) return reject(new Error('Chromium went away'));
      seq += 1;
      const id = seq;
      const timer = setTimeout(() => settle(id, new Error(`${method} took over ${REQUEST_MS / 1000}s`)), REQUEST_MS);
      pending.set(id, { resolve, reject, timer });
      ws.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async expression => {
    const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  };
  const open = async url => {
    // Each from a clean slate: what one scene leaves in localStorage (the
    // account a chat was last made on) would carry into the next.
    await evaluate('try { localStorage.clear(); } catch {}');
    const loaded = new Promise(r => (onLoad = r));
    await send('Page.navigate', { url });
    await Promise.race([loaded, new Promise(r => setTimeout(r, 15000))]);
  };
  await send('Page.enable');
  await send('Runtime.enable');
  return { send, evaluate, open };
}

/** The names a filter gives, or null for all; a name it does not know is an
 *  error, so a typo never passes as a check of nothing. */
function pick(arg, known, what) {
  if (!arg || arg === '-') return null;
  const names = arg.split(',').filter(Boolean);
  const unknown = names.filter(n => !known.includes(n));
  if (unknown.length || !names.length) {
    throw new Error(`no ${what} called ${unknown.join(', ') || '(empty)'}; there are ${known.join(', ')}`);
  }
  return names;
}

async function main() {
  const [out, scenesArg, sizesArg] = process.argv.slice(2);
  if (!out) throw new Error('usage: node scripts/fit-check.mjs <out> [scene,...] [size,...]');
  mkdirSync(out, { recursive: true });

  const cdp = await devtools();
  // The scenes are the demo's own list, read from the page it serves.
  await cdp.open(`${BASE}?demo`);
  const sceneNames = await cdp.evaluate(`(async () => (await import('/src/dev/demo/scenes.ts')).SCENES)()`);
  const all = [{ name: 'main' }, ...sceneNames.filter(s => !s.endsWith('-dark')).map(name => ({ name })), ...EXTRAS];
  const onlyScenes = pick(
    scenesArg,
    all.map(r => r.name),
    'scene'
  );
  const onlySizes = pick(
    sizesArg,
    SIZES.map(z => z.name),
    'size'
  );
  const runs = all.filter(r => !onlyScenes || onlyScenes.includes(r.name));
  const sizes = SIZES.filter(z => !onlySizes || onlySizes.includes(z.name));

  const report = {};
  let failed = 0;
  for (const size of sizes) {
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: size.w,
      height: size.h,
      deviceScaleFactor: size.s,
      mobile: false
    });
    for (const run of runs) {
      await cdp.open(`${BASE}?demo=${run.scene ?? run.name}`);
      // The scenes open what they show on timers of up to a second.
      await new Promise(r => setTimeout(r, 2200));
      const found = [];
      let scrolled = false;
      if (run.setup) {
        try {
          await cdp.evaluate(`(async () => { ${run.setup} })()`);
          await new Promise(r => setTimeout(r, 900));
          if (!(await cdp.evaluate(`!!(${run.opens})`))) throw new Error(`nothing opened (${run.opens})`);
          if (run.then) {
            scrolled = (await cdp.evaluate(`(async () => { ${run.then} })()`)) === true;
            await new Promise(r => setTimeout(r, 900));
          }
        } catch (e) {
          found.push(`could not open it: ${String(e).split('\n')[0]}`);
        }
      }
      const shot = await cdp.send('Page.captureScreenshot', { format: 'png' });
      writeFileSync(`${out}/${run.name}__${size.name}.png`, Buffer.from(shot.data, 'base64'));
      const measured = JSON.parse(await cdp.evaluate(MEASURE));
      found.push(...problems(measured, run.closes && scrolled));
      report[`${run.name}__${size.name}`] = { ...measured, problems: found };
      if (found.length) failed += 1;
      console.log(
        `${found.length ? 'FAIL' : 'ok  '} ${run.name.padEnd(24)} ${size.name.padEnd(13)} ${found.join('; ')}`
      );
    }
  }
  writeFileSync(`${out}/report.json`, JSON.stringify(report, null, 2));
  const total = Object.keys(report).length;
  console.log(failed ? `${failed} of ${total} scene-sizes have problems` : `all ${total} scene-sizes fit`);
  return failed ? 1 : 0;
}

// Chromium goes however the run ends: with its result, an error, or the whole
// run's deadline passing.
const browser = spawn(
  'chromium',
  ['--headless=new', '--no-sandbox', '--disable-gpu', '--remote-debugging-port=9222', 'about:blank'],
  { stdio: 'ignore' }
);
const deadline = setTimeout(() => {
  console.error(`fit check: not done after ${RUN_MS / 60_000} minutes, given up`);
  browser.kill('SIGKILL');
  process.exit(1);
}, RUN_MS);
let code = 1;
try {
  code = await main();
} catch (e) {
  console.error(`fit check: ${e instanceof Error ? e.message : String(e)}`);
} finally {
  clearTimeout(deadline);
  browser.kill('SIGKILL');
}
process.exit(code);
