import { expect, test } from '@playwright/test'

import { createIssueViaUi } from './helpers/issues'
import {
  createRepoViaUi,
  openRepoBySlug,
  waitForDemoRepoNavHydrated,
  waitForRepoIssuesHydrated,
} from './helpers/repo'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('issue create repo route authority', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('transition banner blocks create and POST until route repo hydrates', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug: slugA } = await createRepoViaUi(page)
    const { slug: slugB } = await createRepoViaUi(page)
    await openRepoBySlug(page, slugA)
    await expect(page.getByTestId('repo-subnav-title')).toHaveText(slugA)
    await createIssueViaUi(page, 'fixture on repo A', { slug: slugA })

    let releaseHydration: () => void = () => {}
    const hydrationGate = new Promise<void>((resolve) => {
      releaseHydration = resolve
    })

    await page.route(`**/orgs/demo/repos/${slugB}/**`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await hydrationGate
      await route.continue()
    })

    const bHydrationRequest = page.waitForRequest(
      (req) =>
        req.method() === 'GET' &&
        req.url().includes(`/repos/${slugB}/`) &&
        (req.url().includes('/issues') || req.url().includes('/pull-requests')),
    )
    await page.goto(`/org/demo/repos/${slugB}/issues`, { waitUntil: 'domcontentloaded' })
    await bHydrationRequest
    let issuePostsDuringTransition = 0
    let hydrationReleased = false
    page.on('request', (req) => {
      if (hydrationReleased) {
        return
      }
      if (req.method() !== 'POST' || !req.url().includes(`/repos/${slugB}/issues`)) {
        return
      }
      if (/\/issues\/\d+/.test(req.url())) {
        return
      }
      issuePostsDuringTransition += 1
    })
    await expect(page).toHaveURL(new RegExp(`/repos/${slugB}/issues`))
    await expect(page.getByTestId('repo-subnav-title')).toHaveText(slugB)
    await expect(page.getByTestId('issues-repo-transition')).toBeVisible({ timeout: 10_000 })
    await expect(page.getByTestId('create-issue')).toBeDisabled()
    await page.getByTestId('issue-create-title').fill('must not ship during transition')
    await expect(page.getByTestId('create-issue')).toBeDisabled()
    expect(issuePostsDuringTransition).toBe(0)

    releaseHydration()
    hydrationReleased = true
    await waitForRepoIssuesHydrated(page)

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
