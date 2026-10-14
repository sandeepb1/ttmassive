import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';

// TTM_DEV_PORT points the dev proxy at a throwaway server instead of the real one.
const port = process.env.TTM_DEV_PORT ?? '7681';

export default defineConfig({
  plugins: [tailwindcss()],
  build: { outDir: 'dist', emptyOutDir: true, target: 'es2022' },
  server: {
    proxy: {
      '/api': `http://127.0.0.1:${port}`,
      '/ws': { target: `ws://127.0.0.1:${port}`, ws: true },
    },
  },
});
