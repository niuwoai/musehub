import { expect, test } from '@playwright/test'

import { createRepoViaUi, openRepoDelivery, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('repo delivery console extensions', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('delivery panel shows releases list section', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoDelivery(page, slug)
    await expect(page.getByTestId('delivery-releases')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('delivery-release-list').or(page.getByText(/No releases yet/i))).toBeVisible()
  })
})
