import js from '@eslint/js';
import { defineConfig, globalIgnores } from 'eslint/config';
import reactHooks from 'eslint-plugin-react-hooks';
import globals from 'globals';
import tseslint from 'typescript-eslint';

// Formatting is Prettier's (prettier.config.mjs); nothing here touches layout.

/** The repo scripts' modules that run in a browser page, not in Node. */
const PAGE_SCRIPTS = 'scripts/*-page.mjs';

export default defineConfig(
  // A disable that no longer silences anything goes, rather than hiding the
  // next finding on its line.
  { linterOptions: { reportUnusedDisableDirectives: 'error' } },
  // The Rust crate and build output are not linted here, nor the worktrees
  // Luna's helpers and Claude Code make inside the repo (.claude/worktrees/):
  // checkouts of their own, each with its own config.
  globalIgnores(['dist/', 'src-tauri/', 'test-build/', 'docs/', '.claude/worktrees/']),
  {
    files: ['src/**/*.{ts,tsx}'],
    extends: [js.configs.recommended, tseslint.configs.recommendedTypeChecked, reactHooks.configs.flat.recommended],
    languageOptions: {
      globals: globals.browser,
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname }
    },
    rules: {
      // Destructuring a key out to keep the rest is how the stores drop one.
      '@typescript-eslint/no-unused-vars': ['error', { ignoreRestSiblings: true }]
    }
  },
  // The config files and repo scripts: Node modules outside the app's tsconfig.
  {
    files: ['*.{js,mjs,ts}', 'scripts/**/*.mjs'],
    ignores: [PAGE_SCRIPTS],
    extends: [js.configs.recommended, tseslint.configs.recommended],
    // Named, not guessed: typescript-eslint refuses to pick between candidate
    // roots once another config file (a nested worktree's) has been loaded.
    languageOptions: { globals: globals.node, parserOptions: { tsconfigRootDir: import.meta.dirname } }
  },
  // What a script sends to run in a page: the browser's globals, not Node's.
  {
    files: [PAGE_SCRIPTS],
    extends: [js.configs.recommended, tseslint.configs.recommended],
    languageOptions: { globals: globals.browser, parserOptions: { tsconfigRootDir: import.meta.dirname } }
  }
);
