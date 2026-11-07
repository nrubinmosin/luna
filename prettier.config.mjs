// The options are the ones the code was already written in (single quotes,
// semicolons, no trailing commas, `x => ...` without parentheses), so the
// formatter keeps its style rather than imposing another. 120 columns is where
// most existing lines already sit; a narrower width rewraps thousands of them.
/** @type {import('prettier').Config} */
export default {
  printWidth: 120,
  singleQuote: true,
  trailingComma: 'none',
  arrowParens: 'avoid',
  // The repo stores LF (.gitattributes), but a Windows checkout with
  // core.autocrlf has CRLF in the working tree; `auto` keeps whichever the file
  // has, so `format:check` agrees with both and git normalizes on commit.
  endOfLine: 'auto',
  // The stylesheet quotes its strings (font names, urls) with double quotes.
  overrides: [{ files: '*.css', options: { singleQuote: false } }]
};
