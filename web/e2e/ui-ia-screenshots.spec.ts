import { expect, test } from '@playwright/test'

import { goAudit, goOrgSettings, goRepoDelivery, loginFromHome } from './helpers/navigation'
import { createRepoViaUi, openRepoBySlug, waitForDemoRepoNavHydrated } from './helpers/repo'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('UI IA screenshots', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('capture key routes', async ({ page }) => {
    await loginFromHome(page, password!)
    await expectLoggedInAs(page, 'admin')
    await page.goto('/org/demo/repos')
    await page.screenshot({ path: '/opt/cursor/artifacts/web-login-repos.png', fullPage: true })

    await goOrgSettings(page)
    await page.screenshot({ path: '/opt/cursor/artifacts/web-org-settings.png', fullPage: true })

    await goAudit(page)
    await page.screenshot({ path: '/opt/cursor/artifacts/web-audit.png', fullPage: true })

    await page.goto('/org/demo/repos')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoBySlug(page, slug)
    await page.screenshot({ path: '/opt/cursor/artifacts/web-repo-issues.png', fullPage: true })
    await goRepoDelivery(page)
    await page.screenshot({ path: '/opt/cursor/artifacts/web-repo-delivery.png', fullPage: true })
    await expect(page.getByTestId('delivery-panel')).toBeVisible()
  })
})
