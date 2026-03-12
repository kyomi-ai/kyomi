import { defineConfig } from 'vite';
import { resolve } from 'path';

export default defineConfig({
  build: {
    lib: {
      entry: resolve(__dirname, 'src/index.js'),
      name: 'ChartML',
      fileName: 'index',
      formats: ['es']
    },
    rollupOptions: {
      external: ['d3', 'js-yaml'],
      output: {
        globals: {
          d3: 'd3',
          'js-yaml': 'jsyaml'
        }
      }
    },
    sourcemap: true,
    minify: false
  }
});
