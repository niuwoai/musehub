import { expect, test } from '@playwright/test'
import { expectLoggedInAs } from './helpers/session'
import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('S1 collaboration console', () => {
  test.skip(!password, 'requires bootstrap password')

  test('issue detail, code browse tab, ssh key form', async ({ page }) => {
    await page.goto('/login', { waitUntil: 'domcontentloaded' })
    await page.getByTestId('login-handle-input').fill('admin')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.getByTestId('nav-repo-issues').click()
    await page.getByRole('button', { name: /create issue|创建/i }).click()
    await page.getByTestId('issue-link-1').click({ timeout: 30_000 })
    await expect(page.getByTestId('issue-detail-title')).toBeVisible()
    await page.getByTestId('issue-comment-input').fill('discussion note')
    await page.getByTestId('issue-comment-submit').click()
    await expect(page.getByTestId('issue-comment')).toContainText('discussion note')

    await page.getByTestId('nav-repo-code').click()
    await expect(page.getByTestId('repo-code')).toBeVisible()
    await expect(page.getByTestId('code-pinned-commit')).toContainText('commit=')

    await page.goto('/account/security')
    await expect(page.getByTestId('ssh-keys-section')).toBeVisible()
    await expect(page.getByTestId('ssh-key-add')).toBeVisible()
  })
})
