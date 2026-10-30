import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

const versionFile = fileURLToPath(new URL('../VERSION', import.meta.url))
const appVersion = readFileSync(versionFile, 'utf8').trim()

export default defineConfig({
  plugins: [react()],
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
