import { expect, test } from '@playwright/test'

import { goAccountSecurity, goAudit } from './helpers/navigation'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const TEST_PUB =
  'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM1RneJ7XbTm4iJThlaXp0q55nOJkRyE5PCLUeCKKWpk e2e-console'

test.describe('SSH keys console', () => {
  test.skip(!password, 'requires bootstrap password')

  test('add and revoke key with audit filter ssh_key', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await goAccountSecurity(page)
    await page.getByTestId('ssh-key-name').fill(`pw-key-${Date.now()}`)
    await page.getByTestId('ssh-key-public').fill(TEST_PUB)
    const createResp = page.waitForResponse(
      (r) => r.url().includes('/api/v1/ssh-keys') && r.request().method() === 'POST' && r.ok(),
    )
    await page.getByTestId('ssh-key-add').click()
    const created = await createResp
    const { id, fingerprint } = (await created.json()) as { id: string; fingerprint: string }
    await expect(page.getByTestId('ssh-key-row')).toContainText(fingerprint)

    await goAudit(page)
    await page.getByTestId('audit-resource-type').selectOption('ssh_key')
    await page.getByTestId('audit-apply-filter').click()
    const rows = page.getByTestId('audit-event')
    await expect(rows.first()).toBeVisible({ timeout: 15_000 })
    await expect(rows.first()).toContainText('ssh_key.create')

    await goAccountSecurity(page)
    const revokeResp = page.waitForResponse(
      (r) =>
        r.url().includes(`/api/v1/ssh-keys/${id}`) &&
        r.request().method() === 'DELETE' &&
        r.status() === 204,
    )
    await page.getByTestId(`ssh-key-revoke-${id}`).click()
    await revokeResp
    await expect(page.getByTestId('ssh-key-row').filter({ hasText: fingerprint })).toHaveCount(0)

    await goAudit(page)
    await page.getByTestId('audit-resource-type').selectOption('ssh_key')
    await page.getByTestId('audit-apply-filter').click()
  await expect(page.getByTestId('audit-event').filter({ hasText: 'ssh_key.revoke' })).toHaveCount(
      1,
      { timeout: 15_000 },
    )
  })
})
