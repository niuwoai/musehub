import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

const versionFile = fileURLToPath(new URL('../VERSION', import.meta.url))
const appVersion = readFileSync(versionFile, 'utf8').trim()

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:8080',
      '/git': 'http://127.0.0.1:8080',
    },
  },
  define: {
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(appVersion),
  },
  test: {
    environment: 'jsdom',
    env: {
      VITE_APP_VERSION: appVersion,
    },
  },
})
