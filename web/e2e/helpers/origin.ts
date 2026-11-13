/**
 * Browser-session API calls must target the same origin as Playwright `baseURL`
 * (Vite dev server) so `musehub_session` cookies from `/login` are sent.
 * Direct `:8080` calls omit the session and return 401 — not flaky timing.
 */
export function e2eBrowserApiOrigin(): string {
  const base = process.env.MUSEHUB_E2E_BASE_URL?.replace(/\/$/, '')
  if (base) return base
  if (process.env.MUSEHUB_E2E_API?.includes(':5173')) {
    return process.env.MUSEHUB_E2E_API.replace(/\/$/, '')
  }
  return 'http://127.0.0.1:5173'
}

/** Raw musehub-server origin (health checks, git smart HTTP, runner bootstrap). */
export function e2eServerOrigin(): string {
  const s = process.env.MUSEHUB_E2E_SERVER ?? 'http://127.0.0.1:8080'
  return s.replace(/\/$/, '')
}
