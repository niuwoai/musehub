import { expect, test } from '@playwright/test'

import { goAudit, goOrgSettings, goRepoDelivery, loginFromHome } from './helpers/navigation'
import { createRepoViaUi, openRepoBySlug, waitForDemoRepoNavHydrated } from './helpers/repo'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('UI IA screenshots', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('capture key routes', async ({ page }, testInfo) => {
    const shot = (name: string) => testInfo.outputPath(name)

    await loginFromHome(page, password!)
    await expectLoggedInAs(page, 'admin')
    await page.goto('/org/demo/repos')
    await page.screenshot({ path: shot('web-login-repos.png'), fullPage: true })

    await goOrgSettings(page)
    await page.screenshot({ path: shot('web-org-settings.png'), fullPage: true })

    await goAudit(page)
    await page.screenshot({ path: shot('web-audit.png'), fullPage: true })

    await page.goto('/org/demo/repos')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoBySlug(page, slug)
    await page.screenshot({ path: shot('web-repo-issues.png'), fullPage: true })
    await goRepoDelivery(page)
    await page.screenshot({ path: shot('web-repo-delivery.png'), fullPage: true })
    await expect(page.getByTestId('delivery-panel')).toBeVisible()

    await testInfo.attach('screenshot-manifest', {
      body: JSON.stringify({
        web_login_repos: shot('web-login-repos.png'),
        web_org_settings: shot('web-org-settings.png'),
        web_audit: shot('web-audit.png'),
        web_repo_issues: shot('web-repo-issues.png'),
        web_repo_delivery: shot('web-repo-delivery.png'),
      }),
      contentType: 'application/json',
    })
  })
})
