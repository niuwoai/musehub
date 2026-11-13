import { expect, test } from '@playwright/test'
import { apiJson } from './helpers/s1-api'
import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('Issue delivery console', () => {
  test.skip(!password, 'requires bootstrap password')

  test('detail: criteria, close/reopen, delivery trace gaps', async ({ page }) => {
    await loginAsAdmin(page, password!)
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

  test('assignee select: repo grant members only', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    const member = `e2e-assign-${Date.now()}`
    await apiJson(page, 'POST', '/api/v1/orgs/demo/members', {
      handle: member,
      password: 'E2e-Assignee-Grant-12!',
      role: 'member',
    })
    await apiJson(page, 'POST', `/api/v1/orgs/demo/repos/${slug}/collaborators`, {
      handle: member,
      role: 'maintainer',
    })
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('create-issue').click()
    await page.getByTestId('issue-link-1').click()
    const assignee = page.getByTestId('issue-assignee')
    await expect(assignee).toContainText('admin')
    await expect(assignee).toContainText(member)
    await assignee.selectOption(member)
    await page.getByTestId('issue-save-meta').click()
    await expect(assignee).toHaveValue(member)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await expect(page.getByTestId('issue-assignee-1')).toContainText(`@${member}`)
  })

  test('list: in_progress filter and mark open', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('create-issue').click()
    await page.getByTestId('issue-link-1').click()
    await page.getByTestId('issue-mark-in-progress').click()
    await expect(page.getByTestId('issue-detail-state')).toContainText(/in_progress|进行中/i)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('issue-filter-state').selectOption('in_progress')
    await page.getByTestId('issue-apply-filters').click()
    await expect(page.getByTestId('issue-row-1')).toBeVisible({ timeout: 15_000 })
    await page.getByTestId('issue-filter-state').selectOption('open')
    await page.getByTestId('issue-apply-filters').click()
    await expect(page.getByTestId('issue-row-1')).toHaveCount(0)
    await page.goto(`/org/demo/repos/${slug}/issues/1`)
    await page.getByTestId('issue-mark-open').click()
    await expect(page.getByTestId('issue-detail-state')).toContainText(/open/)
  })

  test('list: assignee filter', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('create-issue').click()
    await page.getByTestId('issue-link-1').click()
    await page.getByTestId('issue-assignee').selectOption('admin')
    await page.getByTestId('issue-save-meta').click()
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('issue-filter-assignee').selectOption('admin')
    await page.getByTestId('issue-apply-filters').click()
    await expect(page.getByTestId('issue-row-1')).toBeVisible({ timeout: 15_000 })
    await page.getByTestId('issue-filter-assignee').selectOption('')
    await page.getByTestId('issue-apply-filters').click()
    const member = `e2e-filter-${Date.now()}`
    await apiJson(page, 'POST', '/api/v1/orgs/demo/members', {
      handle: member,
      password: 'E2e-Filter-Member-12!',
      role: 'member',
    })
    await apiJson(page, 'POST', `/api/v1/orgs/demo/repos/${slug}/collaborators`, {
      handle: member,
      role: 'maintainer',
    })
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('issue-filter-assignee').selectOption(member)
    await page.getByTestId('issue-apply-filters').click()
    await expect(page.getByTestId('issue-row-1')).toHaveCount(0)
  })

  test('list: state badge visible after create', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('create-issue').click()
    await expect(page.getByTestId('issue-row-1')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('issue-state-1')).toHaveText('open')
  })
})
