import { expect, test } from '@playwright/test'

import { goOrgSettings } from './helpers/navigation'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

test.describe('org config and grants UI', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('non-admin cannot patch org settings via API', async ({ request }) => {
    const handle = `cfg-member-${Date.now()}`
    const memberPass = 'Config-Member-E2E-Strong-1!'
    const adminLogin = await request.post(`${apiBase}/api/v1/auth/login`, {
      data: { handle: 'admin', password: password! },
    })
    expect(adminLogin.ok(), `admin login: ${adminLogin.status()}`).toBeTruthy()
    const invite = await request.post(`${apiBase}/api/v1/orgs/demo/members`, {
      data: { handle, password: memberPass, role: 'member' },
    })
    expect(invite.ok()).toBeTruthy()
  await request.post(`${apiBase}/api/v1/auth/logout`)
    const memberLogin = await request.post(`${apiBase}/api/v1/auth/login`, {
      data: { handle, password: memberPass },
    })
    expect(memberLogin.ok()).toBeTruthy()
    const patch = await request.patch(`${apiBase}/api/v1/orgs/demo/settings`, {
      data: { display_name: 'hijacked' },
    })
    expect(patch.status()).toBe(403)
  })

  test('persist org display name without echoing secrets', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await goOrgSettings(page)
  const tokenLike = /mhp_[a-zA-Z0-9]+/
    await expect(page.getByTestId('org-settings')).not.toContainText(tokenLike)
    const name = `Demo Org ${Date.now()}`
    await page.getByTestId('org-display-name').fill(name)
    const settingsPath = '/api/v1/orgs/demo/settings'
    const [saveResponse] = await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'PATCH' &&
          response.url().includes(settingsPath) &&
          response.ok(),
        { timeout: 15_000 },
      ),
      page.getByTestId('save-org-settings').click(),
    ])
    const patched = await saveResponse.json()
    expect(patched.display_name).toBe(name)
    const settings = await page.request.get(`${apiBase}${settingsPath}`)
    expect(settings.ok()).toBeTruthy()
    const body = await settings.json()
    expect(body.display_name).toBe(name)
    expect(JSON.stringify(body)).not.toMatch(tokenLike)
  })
})
