import { expect, test } from '@playwright/test'

import { createRepoViaUi, openRepoDelivery } from './helpers/repo'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('session routing', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('deep link survives refresh when session cookie present', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await page.goto('/org/demo/repos')
    const { slug } = await createRepoViaUi(page)
    const deep = `/org/demo/repos/${slug}/delivery`
    await page.goto(deep)
    await expect(page.getByTestId('nav-repo-delivery')).toBeVisible({ timeout: 15_000 })
    await page.reload()
    await expectLoggedInAs(page, 'admin')
    await expect(page).toHaveURL(new RegExp(`/repos/${slug}/delivery`))
    await expect(page.getByTestId('delivery-panel')).toBeVisible({ timeout: 30_000 })
  })

  test('login returns to safe from path', async ({ page }) => {
    const deep = '/org/demo/repos'
    await page.goto(deep)
    await expect(page.getByTestId('login-handle-input')).toBeVisible({ timeout: 15_000 })
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await expect(page).toHaveURL(/\/org\/demo\/repos/)
  })
})
