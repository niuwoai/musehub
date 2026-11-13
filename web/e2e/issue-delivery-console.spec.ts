import { expect, test } from '@playwright/test'
import { expectLoggedInAs } from './helpers/session'
import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

async function loginAsAdmin(page: import('@playwright/test').Page) {
  await page.goto('/login', { waitUntil: 'domcontentloaded' })
  await page.getByTestId('login-handle-input').fill('admin')
  await page.getByLabel(/password|密码/i).fill(password!)
  await page.getByRole('button', { name: /login|登录/i }).click()
  await expectLoggedInAs(page, 'admin')
}

test.describe('Issue delivery console', () => {
  test.skip(!password, 'requires bootstrap password')

  test('detail: criteria, close/reopen, delivery trace gaps', async ({ page }) => {
    await loginAsAdmin(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('create-issue').click()
    await page.getByTestId('issue-link-1').click()
    await expect(page.getByTestId('issue-assignee')).toBeVisible()
    await expect(page.getByTestId('issue-assignee')).toContainText('admin')
    await expect(page.getByTestId('issue-criteria')).toBeVisible()
    await page.getByTestId('issue-criteria-input').fill('Sign-in works\nAudit entry recorded')
    await page.getByTestId('issue-criteria-save').click()
    await expect(page.getByTestId('issue-criteria-list')).toContainText('Sign-in works')
    await expect(page.getByTestId('issue-delivery-trace')).toBeVisible()
    await expect(page.getByTestId('issue-trace-gaps')).toContainText('no_pull_requests')
    await page.getByTestId('issue-close-reason').fill('completed')
    await page.getByTestId('issue-close').click()
    await expect(page.getByTestId('issue-detail-state')).toContainText(/closed|已关闭/i)
    await expect(page.getByTestId('issue-reopen')).toBeVisible()
    await page.getByTestId('issue-reopen').click()
    await expect(page.getByTestId('issue-close')).toBeVisible({ timeout: 10_000 })
    await expect(page.getByTestId('issue-detail-state')).toContainText(/open/)
  })

  test('list: state badge visible after create', async ({ page }) => {
    await loginAsAdmin(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('create-issue').click()
    await expect(page.getByTestId('issue-row-1')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('issue-state-1')).toHaveText('open')
  })
})
