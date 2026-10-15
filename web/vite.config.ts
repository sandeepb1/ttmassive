import { defineConfig, type Plugin } from 'vite';
import tailwindcss from '@tailwindcss/vite';

// TTM_DEV_PORT points the dev proxy at a throwaway server instead of the real one.
const port = process.env.TTM_DEV_PORT ?? '7681';

/**
 * Preload the two Latin font files. The stylesheet names them, so the browser would only start fetching them once it
 * had downloaded and parsed the CSS and found text that needs them: text swaps fonts after first paint and the terminal
 * measures its cells twice. Their file names are hashed, so the links are written from the finished bundle.
 */
function preloadFonts(): Plugin {
  return {
    name: 'ttm-preload-fonts',
    apply: 'build',
    transformIndexHtml: {
      order: 'post',
      handler(_html, ctx) {
        const names = Object.keys(ctx.bundle ?? {}).filter((f) => /(?:inter|jetbrains-mono)-latin-wght-normal-.+\.woff2$/.test(f));
        return names.map((f) => ({
          tag: 'link',
          attrs: { rel: 'preload', as: 'font', type: 'font/woff2', crossorigin: '', href: `/${f}` },
          injectTo: 'head' as const,
        }));
      },
    },
  };
}

export default defineConfig({
  plugins: [tailwindcss(), preloadFonts()],
  build: { outDir: 'dist', emptyOutDir: true, target: 'es2022' },
  server: {
    proxy: {
      '/api': `http://127.0.0.1:${port}`,
      '/ws': { target: `ws://127.0.0.1:${port}`, ws: true },
    },
  },
});
