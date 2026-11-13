import { expect, test } from '@playwright/test'

import { createIssueViaUi } from './helpers/issues'
import {
  createRepoViaUi,
  openRepoBySlug,
  waitForDemoRepoNavHydrated,
} from './helpers/repo'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('issue create repo route authority', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('create posts to route repo after switching from another repo', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug: slugA } = await createRepoViaUi(page)
    const { slug: slugB } = await createRepoViaUi(page)
    await openRepoBySlug(page, slugA)
    await createIssueViaUi(page, 'fixture on repo A', { slug: slugA })

    const issuesOnB = page.waitForResponse(
      (r) =>
        r.request().method() === 'GET' &&
        r.url().includes(`/repos/${slugB}/issues`) &&
        r.ok(),
    )
    await page.goto(`/org/demo/repos/${slugB}/issues`)
    await issuesOnB
    await expect(page.getByTestId('issue-create-title')).toBeVisible({ timeout: 30_000 })

    const post = page.waitForResponse(
      (r) =>
        r.request().method() === 'POST' &&
        r.ok() &&
        r.url().includes(`/repos/${slugB}/issues`) &&
        !/\/issues\/\d+/.test(r.url()),
    )
    await createIssueViaUi(page, 'fixture on repo B', { slug: slugB })
    const resp = await post
    expect(resp.url()).toContain(`/repos/${slugB}/`)
    expect(resp.url()).not.toContain(`/repos/${slugA}/`)
  })
})
