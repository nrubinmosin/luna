/** A path's last part, by either separator, a trailing one ignored. */
export const baseName = (path: string) =>
  path
    .replace(/[\\/]+$/, '')
    .split(/[\\/]/)
    .pop() || path;
