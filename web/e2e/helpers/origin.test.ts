import { afterEach, describe, expect, it } from 'vitest'

import { e2eBrowserApiOrigin, e2eServerOrigin } from './origin'

describe('e2eBrowserApiOrigin', () => {
  const env = { ...process.env }

  afterEach(() => {
    process.env = { ...env }
  })

  it('prefers MUSEHUB_E2E_BASE_URL for session cookies', () => {
    process.env.MUSEHUB_E2E_BASE_URL = 'http://127.0.0.1:5173/'
    process.env.MUSEHUB_E2E_API = 'http://127.0.0.1:8080'
    expect(e2eBrowserApiOrigin()).toBe('http://127.0.0.1:5173')
  })

  it('defaults to Vite dev origin', () => {
    delete process.env.MUSEHUB_E2E_BASE_URL
    delete process.env.MUSEHUB_E2E_API
    expect(e2eBrowserApiOrigin()).toBe('http://127.0.0.1:5173')
  })
})

describe('e2eServerOrigin', () => {
  it('defaults to musehub-server', () => {
    expect(e2eServerOrigin()).toBe('http://127.0.0.1:8080')
  })
})
