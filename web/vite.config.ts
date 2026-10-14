import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [tailwindcss()],
  build: { outDir: 'dist', emptyOutDir: true, target: 'es2022' },
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:7681',
      '/ws': { target: 'ws://127.0.0.1:7681', ws: true },
    },
  },
});
