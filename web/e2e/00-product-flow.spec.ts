import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'
import { createRepoViaUi, openRepoBySlug, waitForDemoRepoNavHydrated } from './helpers/repo'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('product slice browser flow', () => {
  test.skip(
    !password,
    'requires MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD and running musehub-server on :8080',
  )

  test('login, repo, issue, plan, agent poll', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoBySlug(page, slug)

    await page.getByRole('button', { name: /create issue|创建 issue/i }).click()
    await page.getByRole('button', { name: /accept plan|接受计划/i }).first().click()
    const runAgent = page.getByRole('button', { name: /run agent|运行/i }).first()
    await expect(runAgent).toBeVisible({ timeout: 15_000 })
    await runAgent.click()
    const lastRun = page.getByTestId('last-agent-run')
    await expect(lastRun).toBeVisible({ timeout: 60_000 })
    await expect(lastRun).toContainText(/succeeded/i, { timeout: 120_000 })

    const me = await page.request.get('/api/v1/me')
    expect(me.ok()).toBeTruthy()
  })
})
