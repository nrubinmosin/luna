import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config';

// The app's own Vite config (the React plugin above all), so a test compiles
// the components the way the build does; the DOM is jsdom's.
export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      environment: 'jsdom',
      include: ['src/**/*.test.{ts,tsx}'],
      setupFiles: ['src/test/setup.ts'],
      // `pnpm test:coverage`, for reading locally: no threshold, and the
      // report goes where the test build's files do, which git ignores.
      coverage: {
        provider: 'v8',
        include: ['src/**/*.{ts,tsx}'],
        exclude: ['src/**/*.test.{ts,tsx}', 'src/test/**', 'src/dev/**'],
        reportsDirectory: 'test-build/coverage',
        reporter: ['text', 'html']
      }
    }
  })
);
