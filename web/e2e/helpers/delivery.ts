import { expect, type Page } from '@playwright/test'

import { goRepoDelivery, goRepoIssues, goRepoPulls } from './navigation'

/** Reload repo workspace from API after out-of-band PR/merge updates. */
export async function refreshRepoWorkspace(page: Page): Promise<void> {
  await goRepoIssues(page)
  await page.getByTestId('refresh-repo').click()
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
