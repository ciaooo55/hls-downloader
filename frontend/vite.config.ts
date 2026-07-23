import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const isTauri = Boolean(process.env.TAURI_ENV_PLATFORM)

export default defineConfig({
  plugins: [react()],
  base: isTauri ? './' : '/ui/',
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
