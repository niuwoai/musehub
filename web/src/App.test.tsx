import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import './i18n'

vi.mock('./api', () => ({
  me: vi.fn().mockRejectedValue(new Error('no session')),
  login: vi.fn(),
  logout: vi.fn(),
  listRepos: vi.fn().mockResolvedValue([]),
  listMyTokens: vi.fn().mockResolvedValue([]),
  getRunnerCapabilities: vi.fn().mockResolvedValue(null),
  listAuditEvents: vi.fn().mockResolvedValue([]),
  listRunnerTokens: vi.fn().mockResolvedValue([]),
  listOrgMembers: vi.fn().mockRejectedValue(new Error('forbidden')),
}))

describe('App', () => {
  afterEach(() => {
    cleanup()
  })

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('默认以中文显示产品名与版本', async () => {
    render(<App />)

    await waitFor(() => {
      expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('MuseHub')
    })
    expect(screen.getByText(/未发布的开发版/)).toBeTruthy()
    expect(screen.getByText(/0\.1\.0-rc4/)).toBeTruthy()
    expect(screen.getByRole('button', { name: /登录/ })).toBeTruthy()
  })
})
