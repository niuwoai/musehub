import { expect, test } from '@playwright/test'
import { loginAsAdmin } from './helpers/session'

test.describe('OIDC self-service identity linking', () => {
  const issuerUsername = process.env.MUSEHUB_OIDC_E2E_USERNAME
  const issuerPassword = process.env.MUSEHUB_OIDC_E2E_PASSWORD
  const adminPassword = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

  test.skip(
    !issuerUsername || !issuerPassword || !adminPassword,
    'requires isolated MuseHub and OIDC provider credentials',
  )

  test('links the provider identity to the current account', async ({ page }) => {
    await loginAsAdmin(page, adminPassword!)
    await page.goto('/account/security')

    const identitySection = page.getByTestId('external-identity-section')
    const linkAction = identitySection.locator('a[href^="/api/v1/auth/oidc/link"]')
    await expect(linkAction).toBeVisible()
    await linkAction.click()

    await expect(page.locator('#username')).toBeVisible({ timeout: 20_000 })
    await page.locator('#username').fill(issuerUsername!)
    await page.locator('#password').fill(issuerPassword!)
    const callbackResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/v1/auth/oidc/callback' &&
        response.status() === 303,
      { timeout: 30_000 },
    )
    await page.locator('#kc-login').click()
    await callbackResponse

    await expect(page).toHaveURL(/\/account\/security(?:\?.*)?$/, { timeout: 30_000 })
    await expect(
      identitySection.getByRole('status'),
    ).toContainText(/linked|已绑定/, { timeout: 20_000 })
  })
})
