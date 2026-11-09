import { expect, test } from '@playwright/test'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

test.describe('org config and grants UI', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('persist org display name without echoing secrets', async ({ page }) => {
    await page.goto('/')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expect(page.getByText(/admin/i)).toBeVisible({ timeout: 15_000 })
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    const repo = page.locator('section > ul').first().getByRole('button').last()
    await repo.click()
    await expect(page.getByTestId('org-settings')).toBeVisible({ timeout: 15_000 })
  const tokenLike = /mhp_[a-zA-Z0-9]+/
    await expect(page.getByTestId('org-settings')).not.toContainText(tokenLike)
    const name = `Demo Org ${Date.now()}`
    await page.getByTestId('org-display-name').fill(name)
    await page.getByTestId('save-org-settings').click()
    const settings = await page.request.get(`${apiBase}/api/v1/orgs/demo/settings`)
    expect(settings.ok()).toBeTruthy()
    const body = await settings.json()
    expect(body.display_name).toBe(name)
    expect(JSON.stringify(body)).not.toMatch(tokenLike)
  })
})
