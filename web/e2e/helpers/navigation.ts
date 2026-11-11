import { expect, type Page } from '@playwright/test'

export async function loginFromHome(page: Page, password: string, handle = 'admin') {
  await page.goto('/login')
  await expect(page.getByTestId('login-handle-input')).toBeVisible({ timeout: 20_000 })
  await page.getByTestId('login-handle-input').fill(handle)
  await page.getByLabel(/password|密码/i).fill(password)
  await page.getByRole('button', { name: /login|登录|sign in/i }).click()
}

export async function goNav(page: Page, testId: string) {
  await page.getByTestId(testId).click()
}

export async function goOrgSettings(page: Page) {
  await goNav(page, 'nav-org-settings')
  await expect(page.getByTestId('org-settings')).toBeVisible({ timeout: 15_000 })
}

export async function goOrgMembers(page: Page) {
  await goNav(page, 'nav-members')
  await expect(page.getByTestId('org-members')).toBeVisible({ timeout: 15_000 })
}

export async function goAudit(page: Page) {
  await goNav(page, 'nav-audit')
  await expect(page.getByTestId('audit-panel')).toBeVisible({ timeout: 15_000 })
}

export async function goRunner(page: Page) {
  await goNav(page, 'nav-runner')
  await expect(page.getByTestId('runner-status-panel')).toBeVisible({ timeout: 15_000 })
}

export async function goAccountSecurity(page: Page) {
  await goNav(page, 'nav-account-security')
  await expect(page.getByTestId('account-security')).toBeVisible({ timeout: 15_000 })
}

export async function goAccountTokens(page: Page) {
  await goNav(page, 'nav-account-tokens')
  await expect(page.getByTestId('pat-management')).toBeVisible({ timeout: 15_000 })
}

export async function goRepoPulls(page: Page) {
  await goNav(page, 'nav-repo-pulls')
  await expect(page.getByTestId('repo-pulls-page')).toBeVisible({ timeout: 30_000 })
}

export async function goRepoDelivery(page: Page) {
  await goNav(page, 'nav-repo-delivery')
  await expect(page.getByTestId('delivery-panel')).toBeVisible({ timeout: 30_000 })
}

export async function goRepoIssues(page: Page) {
  await goNav(page, 'nav-repo-issues')
  await expect(page.getByTestId('refresh-repo')).toBeVisible({ timeout: 15_000 })
}
