import { expect, test } from '@playwright/test'

import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('repo code browser', () => {
  test.skip(!password, 'requires bootstrap password')

  test('browse seeded main tree and read text blob', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.getByTestId('nav-repo-code').click()
    await expect(page.getByTestId('repo-code')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('code-pinned-commit')).toBeVisible()
    const oid = await page.getByTestId('code-commit-oid').textContent()
    expect(oid?.length).toBeGreaterThan(20)

    await expect(page.getByTestId('code-tree')).toBeVisible()
    const blobs = page.getByTestId('code-entry-blob')
    const blobCount = await blobs.count()
    if (blobCount > 0) {
      await blobs.first().click()
      await expect(page.getByTestId('code-file-body')).toBeVisible({ timeout: 15_000 })
    } else {
      await expect(page.getByTestId('code-tree').locator('li[role="status"]')).toBeVisible()
    }
  })
})
