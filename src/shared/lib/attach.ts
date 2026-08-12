import { saveMedia, writeSession } from '../../ipc/commands';

const EXT_BY_TYPE: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/bmp': 'bmp',
  'image/svg+xml': 'svg',
  'application/pdf': 'pdf'
};

// A screenshot from the clipboard arrives as a nameless blob; give it something
// readable so the path pasted into the prompt still means something.
const nameFor = (file: File, index: number) => {
  if (file.name && file.name !== 'image.png') return file.name;
  const ext = EXT_BY_TYPE[file.type] || file.type.split('/')[1] || 'bin';
  const kind = file.type.startsWith('image/') ? 'pasted-image' : 'pasted-file';
  return `${kind}${index ? `-${index + 1}` : ''}.${ext}`;
};

const quote = (p: string) => (/[\s"']/.test(p) ? `"${p}"` : p);

// btoa() on a megabyte-sized string blows the argument limit if fed in one go.
const toBase64 = (bytes: Uint8Array) => {
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(bin);
};

/**
 * Copies the given files into the app's media store and types their absolute
 * paths into the chat's PTY, so the CLI can pick them up as attachments.
 * Returns the number of files that made it through.
 */
export async function attachFiles(chatId: string, files: File[]): Promise<number> {
  const paths: string[] = [];

  for (const [i, file] of files.entries()) {
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const path = await saveMedia(chatId, nameFor(file, i), toBase64(bytes));
      if (path) paths.push(path);
    } catch (e) {
      console.warn('[llm-desktop] attach failed', file.name, e);
    }
  }

  if (paths.length) {
    await writeSession(chatId, paths.map(quote).join(' ') + ' ');
  }
  return paths.length;
}

/** Files carried by a paste or drop event, ignoring plain-text payloads. */
export const filesFrom = (dt: DataTransfer | null): File[] => {
  if (!dt) return [];
  if (dt.files?.length) return Array.from(dt.files);
  return Array.from(dt.items ?? [])
    .filter(it => it.kind === 'file')
    .map(it => it.getAsFile())
    .filter((f): f is File => !!f);
};

/**
 * Pulls a screenshot straight out of the OS clipboard.
 *
 * The DOM route is not reliable here: the CLI reads the system clipboard
 * natively when it sees Ctrl+V, but xterm swallows that keystroke, turns it
 * into a browser paste event and forwards only the *text* to the pty — so an
 * image never reaches the CLI at all. Asking the OS directly sidesteps both
 * the missing keystroke and whatever WebView2 chooses to expose to web
 * content. Returns the saved path, or null when the clipboard holds no image.
 */
export async function attachClipboardImage(chatId: string): Promise<string | null> {
  if (!('__TAURI_INTERNALS__' in window)) return null;
  try {
    const { readImage } = await import('@tauri-apps/plugin-clipboard-manager');
    const img = await readImage();
    const [rgba, size] = await Promise.all([img.rgba(), img.size()]);
    if (!rgba.length || !size.width || !size.height) return null;

    // rgba() hands back raw pixels, not an encoded file; paint them into a
    // canvas so the CLI receives an actual PNG.
    const canvas = document.createElement('canvas');
    canvas.width = size.width;
    canvas.height = size.height;
    const ctx = canvas.getContext('2d');
    if (!ctx) return null;
    ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), size.width, size.height), 0, 0);

    const blob = await new Promise<Blob | null>(res => canvas.toBlob(res, 'image/png'));
    if (!blob) return null;

    const bytes = new Uint8Array(await blob.arrayBuffer());
    const path = await saveMedia(chatId, `pasted-image.png`, toBase64(bytes));
    if (path) await writeSession(chatId, quote(path) + ' ');
    return path;
  } catch {
    // No image on the clipboard is the common case, not an error.
    return null;
  }
}
