import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('session handle locator regression', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('session-handle is unique while org members panel lists admin role', async ({ page }) => {
    await page.goto('/')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await expect(page.getByTestId('org-members')).toBeVisible()
    await expect(page.getByTestId('session-handle')).toHaveCount(1)
  })
})
