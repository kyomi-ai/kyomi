import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import removeConsole from 'vite-plugin-remove-console'
import { VitePWA } from 'vite-plugin-pwa'
import path from 'path'
import { fileURLToPath } from 'url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

// Backend API port
const backendUrl = 'http://localhost:8002'

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  plugins: [
    tailwindcss(),
    react(),
    // Remove console.* in production builds
    mode === 'production' && removeConsole(),
    // PWA support — uses injectManifest for custom service worker (push notifications)
    VitePWA({
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.js',
      registerType: 'prompt',
      includeAssets: ['kyomi_icon_192.png', 'kyomi_icon_512.png', 'kyomi-favicon.svg'],
      manifest: {
        name: 'Kyomi - AI-Powered Analytics',
        short_name: 'Kyomi',
        description: 'AI-powered analytics platform for BigQuery with natural language chat, SQL editor, and interactive dashboards',
        theme_color: '#d97706',
        background_color: '#ffffff',
        display: 'standalone',
        icons: [
          {
            src: '/kyomi_icon_192.png',
            sizes: '192x192',
            type: 'image/png',
            purpose: 'any maskable'
          },
          {
            src: '/kyomi_icon_512.png',
            sizes: '512x512',
            type: 'image/png',
            purpose: 'any maskable'
          }
        ]
      },
      injectManifest: {
        maximumFileSizeToCacheInBytes: 5 * 1024 * 1024, // 5 MB
        globPatterns: ['**/*.{js,css,html,ico,png,svg,woff2}'],
      },
      devOptions: {
        enabled: false,
        type: 'module'
      }
    })
  ].filter(Boolean),
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
    // Deduplicate React to prevent multiple instances
    dedupe: ['react', 'react-dom', 'react/jsx-runtime']
  },

  worker: {
    format: 'es'
  },
  server: {
    host: '0.0.0.0',
    port: 5173,
    allowedHosts: [
      'kyomi.ai',
      'app.kyomi.ai',
      'localhost',
    ],
    hmr: {
      path: '/vite-hmr'
      // Protocol, host, and port auto-detect based on current location
    },
    proxy: {
      '/api': {
        target: backendUrl,
        changeOrigin: true,
        secure: false,
        ws: true,  // Enable WebSocket proxying for /api/v1/ws/* endpoints
      },
      '/ws': {
        target: backendUrl,
        changeOrigin: true,
        secure: false,
        ws: true,  // Main app WebSocket at /ws/{user_id}
      },
      '/health': {
        target: backendUrl,
        changeOrigin: true,
        secure: false,
      }
    }
  },

  build: {
    // Production optimizations
    minify: 'terser',
    sourcemap: false, // Set to 'hidden' if you need source maps for debugging

    // Code splitting for better caching
    rollupOptions: {
      output: {
        manualChunks: {
          // Separate ChartML into its own chunk
          'chartml': ['@chartml/core'],
          // Vendor libraries
          'vendor': ['react', 'react-dom', 'react-router-dom'],
          // Monaco editor (large dependency)
          'editor': ['monaco-editor'],
        }
      }
    },

    // Chunk size warning limit
    chunkSizeWarningLimit: 1000,
  },

  // Ensure ChartML is bundled (not treated as external)
  optimizeDeps: {
    include: [
      '@kyomi/chart-header',
      '@chartml/core',
      '@chartml/chart-pie',
      '@chartml/chart-scatter',
      '@chartml/react',
      '@chartml/markdown-it',
      '@chartml/markdown-react',
      'apache-arrow'
    ],
    exclude: [
      '@duckdb/duckdb-wasm'
    ]
  },
}))
