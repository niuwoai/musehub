import { defineConfig } from '@playwright/test'

const webPort = process.env.MUSEHUB_E2E_WEB_PORT ?? '5173'
const webBaseUrl = process.env.MUSEHUB_E2E_BASE_URL ?? `http://127.0.0.1:${webPort}`

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.spec.ts',
  fullyParallel: false,
  workers: 1,
  timeout: 120_000,
  use: {
    baseURL: webBaseUrl,
    trace: 'on-first-retry',
    ...(process.env.MUSEHUB_E2E_BROWSER_CHANNEL === 'chrome' ? { channel: 'chrome' as const } : {}),
  },
  webServer: process.env.MUSEHUB_E2E_SKIP_WEB
    ? undefined
    : {
        command: `yarn dev --host 127.0.0.1 --port ${webPort}`,
        url: webBaseUrl,
        reuseExistingServer: true,
        timeout: 120_000,
      },
})
