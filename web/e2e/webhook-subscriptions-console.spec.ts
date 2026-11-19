import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'

const bootstrapPassword = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('organization webhook subscriptions console', () => {
  test.skip(!bootstrapPassword, 'requires bootstrap admin password and API on :8080')

  test('admin creates, sees once, reloads, and disables a webhook subscription', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(bootstrapPassword!)
    await page.getByRole('button', { name: /login|登录|sign in/i }).click()
    await expectLoggedInAs(page, 'admin')

    await page.getByTestId('nav-webhooks').click()
    await expect(page.getByTestId('org-webhooks-page')).toBeVisible()

    const endpoint = `https://1.1.1.1/musehub-webhook-${Date.now()}`
    await page.getByTestId('webhook-endpoint').fill(endpoint)
    await page.getByLabel('issue.created').check()
    await page.getByTestId('webhook-create').click()

    const oneTimeSecret = page.getByTestId('webhook-created-secret')
    await expect(oneTimeSecret).toBeVisible({ timeout: 15_000 })
    await expect(oneTimeSecret.locator('pre')).toContainText(/^[A-Za-z0-9_-]{43}$/)
    const subscriptionRow = page.getByTestId('webhook-list').locator('li').filter({ hasText: endpoint })
    await expect(subscriptionRow).toContainText(/enabled|已启用/)

    await page.reload()
    await expectLoggedInAs(page, 'admin')
    await expect(page.getByTestId('org-webhooks-page')).toBeVisible()
    await expect(page.getByTestId('webhook-created-secret')).toHaveCount(0)
    await expect(page.getByTestId('webhook-list')).toContainText(endpoint)

    const persistedRow = page.getByTestId('webhook-list').locator('li').filter({ hasText: endpoint })
    await persistedRow.getByRole('button', { name: /disable|停用/i }).click()
    await expect(persistedRow).toContainText(/disabled|已停用/)
    await expect(persistedRow.getByRole('button', { name: /disable|停用/i })).toHaveCount(0)
  })
})
