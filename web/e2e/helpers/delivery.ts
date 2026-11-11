import { expect, type Page } from '@playwright/test'

import { goRepoDelivery, goRepoIssues, goRepoPulls } from './navigation'

/** Wait for pending tab to finish, then switch to all-history and await the list GET (not detail GET). */
export async function switchDeploymentListToAll(page: Page): Promise<void> {
  await expect(page.getByTestId('refresh-deployments')).toBeEnabled({ timeout: 30_000 })
  await expect(page.getByTestId('deployment-view-pending')).toHaveAttribute('aria-pressed', 'true')
  const pendingRefresh = page.waitForResponse(
    (r) =>
      r.request().method() === 'GET' &&
      r.url().includes('state=awaiting_approval') &&
      /\/deployments\?/.test(r.url()),
  )
  await page.getByTestId('refresh-deployments').click()
  await pendingRefresh
  await expect(
    page
      .getByTestId('deployment-list-empty')
      .or(page.locator('[data-testid^="deployment-list-item-"]')),
  ).toBeVisible({ timeout: 30_000 })

  const allList = page.waitForResponse(
    (r) => {
      const url = r.url()
      return (
        r.request().method() === 'GET' &&
        /\/deployments\?/.test(url) &&
        !url.includes('state=awaiting_approval') &&
        r.ok()
      )
    },
    { timeout: 30_000 },
  )
  await expect(page.getByTestId('deployment-view-all')).toBeEnabled()
  await page.getByTestId('deployment-view-all').click()
  await allList
  await expect(page.getByTestId('deployment-view-all')).toHaveAttribute('aria-pressed', 'true')
  await expect(
    page.getByRole('status', { name: /loading deployments|正在加载部署/i }),
  ).toHaveCount(0)
}

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
