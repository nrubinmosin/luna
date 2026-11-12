import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // test-build/ holds the test build's CLIs, Rust targets and shots, gigabytes
    // the dev server would crawl at start: through Docker's bind mount that took
    // the fit check's first call past its 30 s. The worktrees made inside the
    // repo (.claude/worktrees/) are whole checkouts of their own, test-build/
    // and all.
    watch: { ignored: ['**/src-tauri/**', '**/test-build/**', '**/.claude/worktrees/**'] }
  }
});
