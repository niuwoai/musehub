import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

const versionFile = fileURLToPath(new URL('../VERSION', import.meta.url))
const appVersion = readFileSync(versionFile, 'utf8').trim()
const e2eServer = process.env.MUSEHUB_E2E_SERVER ?? 'http://127.0.0.1:8080'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': e2eServer,
      '/git': e2eServer,
    },
  },
  define: {
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(appVersion),
  },
  test: {
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx', 'e2e/helpers/**/*.test.ts'],
    exclude: ['**/node_modules/**', '**/dist/**', 'e2e/**/*.spec.ts'],
    environment: 'jsdom',
    env: {
      VITE_APP_VERSION: appVersion,
    },
  },
})
