import { expect, type Page } from '@playwright/test'

import { goRepoDelivery, goRepoIssues, goRepoPulls } from './navigation'

/** Reload repo workspace from API after out-of-band PR/merge updates. */
export async function refreshRepoWorkspace(page: Page): Promise<void> {
  await goRepoIssues(page)
  const listReady = page.waitForResponse(
    (r) =>
      r.request().method() === 'GET' &&
      r.ok() &&
      /\/repos\/[^/]+\/pull-requests/.test(r.url()),
    { timeout: 30_000 },
  )
  await page.getByTestId('refresh-repo').click()
  await listReady
}

export async function refreshRepoAndOpenDelivery(page: Page): Promise<void> {
  await refreshRepoWorkspace(page)
  await goRepoDelivery(page)
}

export async function expectMergedPullOnPullsPage(page: Page): Promise<void> {
  await refreshRepoWorkspace(page)
  await goRepoPulls(page)
  await expect(page.getByTestId('pull-request-list')).toContainText(/merged/i, {
    timeout: 30_000,
  })
}
