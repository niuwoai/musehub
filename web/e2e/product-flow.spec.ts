import { expect, test } from '@playwright/test'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

test.describe('product slice browser flow', () => {
  test.skip(
    !password,
    'requires MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD and running musehub-server on :8080',
  )

  test('login, repo, issue, plan, agent poll', async ({ page }) => {
    await page.goto('/')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expect(page.getByText(/admin/i)).toBeVisible({ timeout: 15_000 })
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await expect(page.locator('ul li button').first()).toBeVisible({
      timeout: 10_000,
    })

    const cookie = (await page.context().cookies()).find((c) =>
      c.name.includes('musehub'),
    )
    expect(cookie).toBeTruthy()

    const repoSlug = await page.locator('ul li button').first().textContent()
    expect(repoSlug).toBeTruthy()
    await page.locator('ul li button').first().click()

    await page
      .getByRole('button', { name: /create issue|创建 issue/i })
      .click()
    await page.getByRole('button', { name: /accept plan|接受计划/i }).click()
    await page.getByRole('button', { name: /run agent|运行/i }).first().click()
    await expect(page.getByTestId('last-agent-run')).toContainText(/succeeded/i, {
      timeout: 60_000,
    })

    const me = await page.request.get(`${apiBase}/api/v1/me`)
    expect(me.ok()).toBeTruthy()
  })
})
