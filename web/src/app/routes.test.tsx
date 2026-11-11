import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { AppRouter } from './AppRouter'
import { WorkspaceProvider } from './WorkspaceProvider'
import '../i18n'

let meResolve: (v: { handle: string }) => void
let meReject: () => void

vi.mock('../api', () => ({
  me: vi.fn(
    () =>
      new Promise<{ handle: string }>((resolve, reject) => {
        meResolve = resolve
        meReject = reject
      }),
  ),
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

  it('shows boot screen while session restores on deep link', async () => {
    renderAt('/org/demo/repos/foo/delivery')
    expect(screen.getByTestId('session-boot')).toBeTruthy()
    meResolve({ handle: 'admin' })
    await waitFor(() => {
      expect(screen.queryByTestId('session-boot')).toBeNull()
    })
    expect(screen.getByTestId('nav-repo-delivery')).toBeTruthy()
  })

  it('redirects unauthenticated /org/demo/repos to login after restore', async () => {
    renderAt('/org/demo/repos')
    meReject()
    await waitFor(() => {
      expect(screen.getByTestId('login-handle-input')).toBeTruthy()
    })
    expect(screen.queryByTestId('repo-list')).toBeNull()
  })

  it('renders login page at /login after anonymous restore', async () => {
    renderAt('/login')
    meReject()
    await waitFor(() => {
      expect(screen.getByTestId('login-handle-input')).toBeTruthy()
    })
    expect(screen.getByRole('button', { name: /登录|Sign in/i })).toBeTruthy()
  })

  it('shows nav when session restores on repos route', async () => {
    renderAt('/org/demo/repos')
    meResolve({ handle: 'admin' })
    await waitFor(() => {
      expect(screen.getByTestId('nav-repos')).toBeTruthy()
    })
    expect(screen.getByTestId('session-handle').textContent).toBe('admin')
  })

  it('404 for unknown org slug', async () => {
    renderAt('/org/other/repos')
    meResolve({ handle: 'admin' })
    await waitFor(() => {
      expect(screen.getByTestId('not-found')).toBeTruthy()
    })
  })
})
