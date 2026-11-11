import { expect, test } from '@playwright/test'

import {
  goAccountTokens,
  goAudit,
  goOrgMembers,
  goOrgSettings,
  goRepoDelivery,
  goRepoPulls,
  goRunner,
  loginFromHome,
} from './helpers/navigation'
import { createRepoViaUi, openRepoBySlug, waitForDemoRepoNavHydrated } from './helpers/repo'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('UI IA screenshots', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('capture key routes', async ({ page }, testInfo) => {
    const shot = (name: string) => testInfo.outputPath(name)

    await page.goto('/login')
    await page.screenshot({ path: shot('web-login-form.png'), fullPage: true })

    await loginFromHome(page, password!)
    await expectLoggedInAs(page, 'admin')
    await page.goto('/org/demo/repos')
    await page.screenshot({ path: shot('web-repos.png'), fullPage: true })

    await goOrgSettings(page)
    await page.screenshot({ path: shot('web-org-settings.png'), fullPage: true })

    await goOrgMembers(page)
    await page.screenshot({ path: shot('web-org-members.png'), fullPage: true })

    await goAudit(page)
    await page.screenshot({ path: shot('web-audit.png'), fullPage: true })

    await goRunner(page)
    await page.screenshot({ path: shot('web-runner.png'), fullPage: true })

    await goAccountTokens(page)
    await page.screenshot({ path: shot('web-account-tokens.png'), fullPage: true })

    await page.goto('/org/demo/repos')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoBySlug(page, slug)
    await page.screenshot({ path: shot('web-repo-issues.png'), fullPage: true })
    await goRepoPulls(page)
    await page.screenshot({ path: shot('web-repo-pulls.png'), fullPage: true })
    await goRepoDelivery(page)
    await page.screenshot({ path: shot('web-repo-delivery.png'), fullPage: true })
    await expect(page.getByTestId('delivery-panel')).toBeVisible()

    await page.goto('/org/unknown-org/repos')
    await expect(page.getByTestId('not-found')).toBeVisible({ timeout: 15_000 })
    await page.screenshot({ path: shot('web-not-found.png'), fullPage: true })

    const manifest = {
      web_login_form: shot('web-login-form.png'),
      web_repos: shot('web-repos.png'),
      web_org_settings: shot('web-org-settings.png'),
      web_org_members: shot('web-org-members.png'),
      web_audit: shot('web-audit.png'),
      web_runner: shot('web-runner.png'),
      web_account_tokens: shot('web-account-tokens.png'),
      web_repo_issues: shot('web-repo-issues.png'),
      web_repo_pulls: shot('web-repo-pulls.png'),
      web_repo_delivery: shot('web-repo-delivery.png'),
      web_not_found: shot('web-not-found.png'),
    }
    await testInfo.attach('screenshot-manifest', {
      body: JSON.stringify(manifest, null, 2),
      contentType: 'application/json',
    })
  })
})
