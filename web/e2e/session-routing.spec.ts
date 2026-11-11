import { expect, test } from '@playwright/test'

import { createRepoViaUi, openRepoDelivery, openRepoBySlug, waitForDemoRepoNavHydrated } from './helpers/repo'
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
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    const deep = `/org/demo/repos/${slug}/delivery`
    await page.goto(deep)
    await expect(page.getByTestId('nav-repo-delivery')).toBeVisible({ timeout: 15_000 })
    await page.reload()
    await expectLoggedInAs(page, 'admin')
    await expect(page).toHaveURL(new RegExp(`/repos/${slug}/delivery`))
    await expect(page.getByTestId('delivery-panel')).toBeVisible({ timeout: 30_000 })
  })

  test('login returns to safe from path on repo issues deep link', async ({ page, context }) => {
    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await page.goto('/org/demo/repos')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    const deep = `/org/demo/repos/${slug}/issues`
    await context.clearCookies()
    await page.goto(deep)
    await expect(page.getByTestId('login-handle-input')).toBeVisible({ timeout: 15_000 })
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await expect(page).toHaveURL(new RegExp(`/repos/${slug}/issues`))
    await expect(page.getByTestId('nav-repo-issues')).toBeVisible({ timeout: 15_000 })
  })

  test('browser back and forward keep routed repo context', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await page.goto('/org/demo/repos')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoBySlug(page, slug)
    await page.getByTestId('nav-repo-pulls').click()
    await expect(page).toHaveURL(new RegExp(`/repos/${slug}/pulls`))
    await page.getByTestId('nav-repo-delivery').click()
    await expect(page).toHaveURL(new RegExp(`/repos/${slug}/delivery`))
    await page.goBack()
    await expect(page).toHaveURL(new RegExp(`/repos/${slug}/pulls`))
    await page.goForward()
    await expect(page).toHaveURL(new RegExp(`/repos/${slug}/delivery`))
    await expect(page.getByTestId('delivery-panel')).toBeVisible({ timeout: 30_000 })
  })
})
