import { cleanup, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { AppRouter } from './AppRouter'
import { WorkspaceProvider } from './WorkspaceProvider'
import '../i18n'

vi.mock('../api', () => ({
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

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <WorkspaceProvider>
        <AppRouter />
      </WorkspaceProvider>
    </MemoryRouter>,
  )
}

describe('AppRouter', () => {
  afterEach(() => {
    cleanup()
  })

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('redirects unauthenticated /org/demo/repos to login', async () => {
    renderAt('/org/demo/repos')
    expect(await screen.findByTestId('login-handle-input')).toBeTruthy()
    expect(screen.queryByTestId('repo-list')).toBeNull()
  })

  it('renders login page at /login', () => {
    renderAt('/login')
    expect(screen.getByTestId('login-handle-input')).toBeTruthy()
    expect(screen.getByRole('button', { name: /登录|Sign in/i })).toBeTruthy()
  })

  it('shows nav links when session is mocked', async () => {
    const { me } = await import('../api')
    vi.mocked(me).mockResolvedValue({ handle: 'admin' } as Awaited<ReturnType<typeof me>>)
    renderAt('/org/demo/repos')
    expect(await screen.findByTestId('nav-repos')).toBeTruthy()
    expect(screen.getByTestId('nav-audit')).toBeTruthy()
    expect(screen.getByTestId('session-handle').textContent).toBe('admin')
  })
})
