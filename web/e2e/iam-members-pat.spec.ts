import { expect, test } from '@playwright/test'

import { goAccountTokens, goAudit, goOrgMembers } from './helpers/navigation'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'
const fullPatLike = /mhp_[a-zA-Z0-9]{24,}/

test.describe('org members and PAT console', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('add member, mint PAT once, revoke without secret leak on reload', async ({ page }) => {
    const handle = `iam-ui-${Date.now()}`
    const memberPass = 'Iam-Ui-Console-E2E-12!'

    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await goOrgMembers(page)

    await page.getByTestId('member-handle').fill(handle)
    await page.getByTestId('member-password').fill(memberPass)
    await page.getByTestId('invite-member').click()
    await expect(page.getByTestId('org-members-list')).toContainText(handle, { timeout: 15_000 })

    const patName = `ui-pat-${Date.now()}`
    await goAccountTokens(page)
    expect(await page.locator('body').innerText()).not.toMatch(/mhp_[a-zA-Z0-9]{24,}/)
    await page.getByTestId('pat-name').fill(patName)
    await page.getByTestId('create-pat').click()
    const minted = page.getByTestId('pat-minted-once')
    await expect(minted).toBeVisible({ timeout: 10_000 })
    const mintedText = await minted.innerText()
    expect(mintedText).toMatch(fullPatLike)

    await page.reload()
    await expectLoggedInAs(page, 'admin')
    await expect(page.getByTestId('pat-minted-once')).toHaveCount(0)
    const bodyAfterReload = await page.locator('body').innerText()
    expect(bodyAfterReload).not.toMatch(fullPatLike)

    await page.getByTestId('logout').click()
    await expect(page.getByTestId('session-handle')).toHaveCount(0)
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    expect(await page.locator('body').innerText()).not.toMatch(fullPatLike)

    const list = await page.request.get(`${apiBase}/api/v1/tokens`)
    expect(list.ok()).toBeTruthy()
    const tokens = (await list.json()) as { id: string; name: string }[]
    const row = tokens.find((t) => t.name === patName)
    expect(row?.id).toBeTruthy()
    await goAccountTokens(page)
    await page.getByTestId(`revoke-pat-${row!.id}`).click()
    await expect(page.getByTestId('pat-masked-list')).not.toContainText(patName)

    const audit = await page.request.get(`${apiBase}/api/v1/orgs/demo/audit/events?limit=50`)
    expect(audit.ok()).toBeTruthy()
    const events = (await audit.json()) as { action: string }[]
    const actions = events.map((e) => e.action)
    expect(actions).toContain('org.member.invite')
    expect(actions).toContain('token.create')
    expect(actions).toContain('token.revoke')
    expect(JSON.stringify(events)).not.toMatch(fullPatLike)

    await goAudit(page)
    await expect(page.getByTestId('audit-panel')).toBeVisible()
    const auditBody = await page.getByTestId('audit-panel').innerText()
    expect(auditBody).not.toMatch(fullPatLike)
  })
})
