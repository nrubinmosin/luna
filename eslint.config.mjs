import js from '@eslint/js';
import { defineConfig, globalIgnores } from 'eslint/config';
import reactHooks from 'eslint-plugin-react-hooks';
import globals from 'globals';
import tseslint from 'typescript-eslint';

// Formatting is Prettier's (prettier.config.mjs); nothing here touches layout.
export default defineConfig(
  // A disable that no longer silences anything goes, rather than hiding the
  // next finding on its line.
  { linterOptions: { reportUnusedDisableDirectives: 'error' } },
  // The Rust crate and build output are not linted here.
  globalIgnores(['dist/', 'src-tauri/', 'test-build/', 'docs/']),
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
  // The config files themselves: Node modules outside the app's tsconfig.
  {
    files: ['*.{js,mjs,ts}'],
    extends: [js.configs.recommended, tseslint.configs.recommended],
    languageOptions: { globals: globals.node }
  }
);
