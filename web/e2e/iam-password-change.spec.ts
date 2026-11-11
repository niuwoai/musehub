import { expect, request as playwrightRequest, test } from '@playwright/test'

import { goAccountSecurity } from './helpers/navigation'
import { expectLoggedInAs } from './helpers/session'

const adminPassword = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

test.describe('self-service password change', () => {
  test.skip(!adminPassword, 'requires bootstrap password and API on :8080')

  test('member rotates password in console; other session revoked', async ({ page, request }) => {
    const handle = `pwd-ui-${Date.now()}`
    const initialPass = 'Pwd-Ui-Initial-12!'
    const rotatedPass = 'Pwd-Ui-Rotated-99!'

    const adminLogin = await request.post(`${apiBase}/api/v1/auth/login`, {
      data: { handle: 'admin', password: adminPassword! },
    })
    expect(adminLogin.ok()).toBeTruthy()
    const invite = await request.post(`${apiBase}/api/v1/orgs/demo/members`, {
      data: { handle, password: initialPass, role: 'member' },
    })
    expect(invite.ok()).toBeTruthy()

    const staleCtx = await playwrightRequest.newContext({ baseURL: apiBase })
    const staleLogin = await staleCtx.post('/api/v1/auth/login', {
      data: { handle, password: initialPass },
    })
    expect(staleLogin.ok()).toBeTruthy()

    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(initialPass)
    await page.getByTestId('login-handle-input').fill(handle)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, handle)
    await goAccountSecurity(page)

    await page.getByTestId('password-current').fill(initialPass)
    await page.getByTestId('password-new').fill(rotatedPass)
    await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes('/api/v1/auth/change-password') && r.request().method() === 'POST',
      ),
      page.getByTestId('change-password').click(),
    ])

    const staleMe = await staleCtx.get('/api/v1/me')
    expect(staleMe.status()).toBe(401)
    await staleCtx.dispose()

    await page.getByTestId('logout').click()
    await expect(page.getByTestId('session-handle')).toHaveCount(0)

    await page.getByLabel(/password|密码/i).fill(rotatedPass)
    await page.getByTestId('login-handle-input').fill(handle)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, handle)

    const audit = await page.request.get(`${apiBase}/api/v1/orgs/demo/audit/events?limit=40`)
    expect(audit.ok()).toBeTruthy()
    const events = (await audit.json()) as { action: string }[]
    expect(events.map((e) => e.action)).toContain('auth.password_change')
    const auditBody = JSON.stringify(events)
    expect(auditBody).not.toContain(initialPass)
    expect(auditBody).not.toContain(rotatedPass)
  })
})
