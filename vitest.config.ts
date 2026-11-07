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
      setupFiles: ['src/test/setup.ts']
    }
  })
);
