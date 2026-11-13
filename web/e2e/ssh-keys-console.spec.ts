import { expect, test } from '@playwright/test'

import { goAccountSecurity } from './helpers/navigation'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const TEST_PUB =
  'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM1RneJ7XbTm4iJThlaXp0q55nOJkRyE5PCLUeCKKWpk e2e-console'

test.describe('SSH keys console', () => {
  test.skip(!password, 'requires bootstrap password')

  test('add and revoke key; audit via self-scoped API only', async ({ page }) => {
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

    const selfAudit = await page.request.get('/api/v1/me/audit/events?resource_type=ssh_key&limit=20')
    expect(selfAudit.ok()).toBeTruthy()
    const body = await selfAudit.text()
    expect(body).not.toContain('AAAAC3NzaC1lZDI1NTE5')
    const events = (await selfAudit.json()) as Array<{
      action: string
      resource_id: string
      org_id: string | null
    }>
    const createEv = events.find((e) => e.action === 'ssh_key.create' && e.resource_id === id)
    expect(createEv).toBeTruthy()
    expect(createEv!.org_id).toBeNull()

    const orgAudit = await page.request.get('/api/v1/orgs/demo/audit/events?resource_type=ssh_key&limit=20')
    expect(orgAudit.ok()).toBeTruthy()
    const orgEvents = (await orgAudit.json()) as Array<{ resource_type: string; resource_id: string }>
    expect(orgEvents.some((e) => e.resource_type === 'ssh_key' && e.resource_id === id)).toBeFalsy()

    const revokeResp = page.waitForResponse(
      (r) =>
        r.url().includes(`/api/v1/ssh-keys/${id}`) &&
        r.request().method() === 'DELETE' &&
        r.status() === 204,
    )
    await page.getByTestId(`ssh-key-revoke-${id}`).click()
    await revokeResp
    await expect(page.getByTestId('ssh-key-row').filter({ hasText: fingerprint })).toHaveCount(0)

    const after = await page.request.get('/api/v1/me/audit/events?resource_type=ssh_key&limit=20')
    const afterEvents = (await after.json()) as Array<{ action: string; resource_id: string }>
    expect(
      afterEvents.some((e) => e.action === 'ssh_key.revoke' && e.resource_id === id),
    ).toBeTruthy()
  })
})
