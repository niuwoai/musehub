import { expect, test } from '@playwright/test'

import { e2eServerOrigin } from './helpers/origin'
import { apiJson } from './helpers/s1-api'
import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('IAM grants console', () => {
  test.skip(!password, 'requires bootstrap password')

  test('grant and revoke on repo settings without mutating org defaults', async ({
    page,
    playwright,
  }) => {
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

    await page.goto(`/org/demo/repos/${slug}/settings`)
    await expect(page.getByTestId('repo-settings-panel')).toBeVisible({ timeout: 15_000 })

    await page.getByTestId('repo-grant-handle').fill(member)
    await page.getByTestId('repo-grant-role').selectOption('reader')
    await page.getByTestId('repo-grant-submit').click()
    const memberRow = page.getByTestId('repo-grant-row').filter({ hasText: member })
    await expect(memberRow).toHaveCount(1, { timeout: 15_000 })
    await expect(memberRow).toContainText('reader')

    const memberApi = await playwright.request.newContext({
      baseURL: e2eServerOrigin(),
    })
    try {
      const memberLogin = await memberApi.post('/api/v1/auth/login', {
        data: { handle: member, password: memberPass },
      })
      expect(memberLogin.ok()).toBeTruthy()
      const memberIssues = await memberApi.get(`/api/v1/orgs/demo/repos/${slug}/issues`)
      expect(memberIssues.ok()).toBeTruthy()

      await expect(memberRow).toHaveCount(1)
      await memberRow.getByTestId('repo-grant-revoke').click()
      await expect(memberRow).toHaveCount(0, { timeout: 15_000 })

      await memberApi.post('/api/v1/auth/logout')
      const memberLogin2 = await memberApi.post('/api/v1/auth/login', {
        data: { handle: member, password: memberPass },
      })
      expect(memberLogin2.ok()).toBeTruthy()
      const denied = await memberApi.get(`/api/v1/orgs/demo/repos/${slug}/issues`)
      expect(denied.status()).toBe(403)
    } finally {
      await memberApi.dispose()
    }
  })
})
