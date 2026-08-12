import { saveMedia } from '../../ipc/commands';
import { writeSession } from '../../ipc/commands';

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
