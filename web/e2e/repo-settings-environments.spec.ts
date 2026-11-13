import { expect, test } from '@playwright/test'

import { createRepoViaUi, openRepoSettings, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('repo settings environments', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('maintainer adds staging environment', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoSettings(page, slug)
    await page.getByTestId('repo-env-slug').fill('staging')
    await page.getByTestId('repo-env-name').fill('Staging')
    const createResp = page.waitForResponse(
      (r) =>
        r.url().includes('/environments') &&
        r.request().method() === 'POST' &&
        r.ok(),
    )
    await page.getByTestId('repo-env-submit').click()
    await createResp
    await expect(page.getByTestId('env-staging')).toBeVisible()
    await expect(page.getByTestId('env-staging')).toContainText(/approval required|需审批/i)
    const toggle = page.waitForResponse(
      (r) =>
        r.url().includes('/environments/staging') &&
        r.request().method() === 'PATCH' &&
        r.ok(),
    )
    await page.getByTestId('env-toggle-approval-staging').click()
    await toggle
    await expect(page.getByTestId('env-staging')).not.toContainText(/approval required|需审批/i)
  })
})
