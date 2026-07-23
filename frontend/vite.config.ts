import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

const rootDir = path.dirname(fileURLToPath(import.meta.url))
const isTauri = Boolean(process.env.TAURI_ENV_PLATFORM)

export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: isTauri ? './' : '/ui/',
  resolve: {
    alias: {
      '@': path.resolve(rootDir, 'src'),
    },
  },
  server: {
    port: 1420,
    strictPort: true,
    proxy: {
      '/api': 'http://127.0.0.1:8765',
    },
  },
  build: {
    outDir: 'dist',
  },
})
