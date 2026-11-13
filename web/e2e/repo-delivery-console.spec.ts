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
    await expect(page.getByTestId('delivery-releases')).toBeVisible()
    await expect(page.getByTestId('delivery-releases-empty')).toBeVisible({ timeout: 15_000 })
  })

  test('renders release rows when GET returns data', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.route(`**/api/v1/orgs/demo/repos/${slug}/releases`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([
          {
            id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
            version: '1.0.0',
            tag_name: 'v0.9.0',
            commit_oid: 'a'.repeat(40),
            pipeline_run_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
            state: 'published',
          },
        ]),
      })
    })
    await openRepoDelivery(page, slug)
    await expect(page.getByTestId('delivery-release-list')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('release-row-v0.9.0')).toBeVisible()
    await expect(page.getByTestId('delivery-releases-error')).toHaveCount(0)
  })

  test('shows error when releases GET fails', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.route(`**/api/v1/orgs/demo/repos/${slug}/releases`, (route) =>
      route.fulfill({ status: 500, body: 'fail' }),
    )
    await openRepoDelivery(page, slug)
    await expect(page.getByTestId('delivery-releases-error')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('delivery-releases-empty')).toHaveCount(0)
  })
})
