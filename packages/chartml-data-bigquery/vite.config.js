import { defineConfig } from 'vite';
import { resolve } from 'path';

export default defineConfig({
  build: {
    lib: {
      entry: resolve(__dirname, 'src/index.js'),
      name: 'ChartMLBigQuery',
      fileName: 'index',
      formats: ['es']
    },
    rollupOptions: {
      external: ['@chartml/core'],
      output: {
        globals: {
          '@chartml/core': 'ChartMLCore'
        }
      }
    },
    sourcemap: true
  }
});
