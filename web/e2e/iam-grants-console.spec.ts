import { expect, test } from '@playwright/test'

import { apiJson } from './helpers/s1-api'
import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'
import { goOrgSettings } from './helpers/navigation'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

test.describe('IAM grants console', () => {
  test.skip(!password, 'requires bootstrap password')

  test('org default role, grant and revoke on repo settings', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)

    const member = `grant-ui-${Date.now()}`
    const memberPass = 'Grant-Ui-Console-E2E-12!'
    await apiJson(page, 'POST', '/api/v1/orgs/demo/members', {
      handle: member,
      password: memberPass,
      role: 'member',
    })

    await goOrgSettings(page)
    await page.getByTestId('org-default-collab-role').selectOption('developer')
    await page.getByTestId('save-org-settings').click()
    const settings = await apiJson<{ preferences: { default_collaborator_role: string } }>(
      page,
      'GET',
      '/api/v1/orgs/demo/settings',
    )
    expect(settings.preferences.default_collaborator_role).toBe('developer')

    await page.getByTestId('nav-repo-settings').click()
    await expect(page.getByTestId('repo-settings-panel')).toBeVisible({ timeout: 15_000 })

    await page.getByTestId('repo-grant-handle').fill(member)
    await page.getByTestId('repo-grant-role').selectOption('reader')
    await page.getByTestId('repo-grant-submit').click()
    await expect(page.getByTestId('repo-grant-row')).toContainText(member, { timeout: 15_000 })
    await expect(page.getByTestId('repo-grant-row')).toContainText('reader')

    const memberLogin = await page.request.post(`${apiBase}/api/v1/auth/login`, {
      data: { handle: member, password: memberPass },
    })
    expect(memberLogin.ok()).toBeTruthy()
    const memberIssues = await page.request.get(
      `${apiBase}/api/v1/orgs/demo/repos/${slug}/issues`,
    )
    expect(memberIssues.ok()).toBeTruthy()

    await page.getByTestId('repo-grant-revoke').first().click()
    await expect(page.getByTestId('repo-grant-row')).toHaveCount(0, { timeout: 15_000 })

    await page.request.post(`${apiBase}/api/v1/auth/logout`)
    const memberLogin2 = await page.request.post(`${apiBase}/api/v1/auth/login`, {
      data: { handle: member, password: memberPass },
    })
    expect(memberLogin2.ok()).toBeTruthy()
    const denied = await page.request.get(`${apiBase}/api/v1/orgs/demo/repos/${slug}/issues`)
    expect(denied.status()).toBe(403)
  })
})
