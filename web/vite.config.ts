import { defineConfig } from 'vitest/config';
import { resolve } from 'node:path';

// The build goes where the Rust build expects it (build.rs reads the same variable).
const outDir = process.env.HERDR_PLANNER_DIST || resolve(__dirname, 'dist');

export default defineConfig({
  root: __dirname,
  publicDir: resolve(__dirname, 'public'),
  build: {
    outDir,
    emptyOutDir: true,
    target: 'es2022',
    sourcemap: false,
    chunkSizeWarningLimit: 1200,
    rollupOptions: {
      input: {
        index: resolve(__dirname, 'index.html'),
        login: resolve(__dirname, 'login.html'),
        m: resolve(__dirname, 'm.html'),
      },
      output: {
        manualChunks: {
          three: ['three'],
        },
      },
    },
  },
  server: {
    proxy: { '/api': 'http://127.0.0.1:7117' },
  },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
