/// <reference types="vitest/config" />

import { defineConfig } from 'vite';
import { resolve } from 'path';
import { playwright } from '@vitest/browser-playwright';

export default defineConfig({
  resolve: {
    conditions: ['production'], // Suppress Lit dev-mode warnings in tests
  },
  build: {
    lib: {
      entry: resolve(import.meta.dirname!, 'src/index.ts'),
      name: 'SemaUI',
      formats: ['es'],
      fileName: 'sema-ui',
    },
    minify: 'esbuild',
  },
  test: {
    projects: [
      {
        extends: true, // inherit resolve.conditions so Lit dev-mode warnings stay suppressed
        test: {
          name: 'browser',
          include: ['tests/**/*.test.ts'],
          exclude: ['tests/tokens.test.ts'],
          browser: {
            enabled: true,
            headless: true,
            provider: playwright(),
            instances: [{ browser: 'chromium' }],
          },
        },
      },
      {
        extends: true,
        test: {
          name: 'node',
          include: ['tests/tokens.test.ts', 'tests/sema-tokenize.test.ts'],
          environment: 'node',
        },
      },
    ],
  },
});
