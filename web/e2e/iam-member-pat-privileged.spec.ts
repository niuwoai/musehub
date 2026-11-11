import { expect, test } from '@playwright/test'

import { goAccountTokens } from './helpers/navigation'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'
const fullPatLike = /mhp_[a-zA-Z0-9]{24,}/

test.describe('ordinary member PAT console', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('member can mint and revoke PAT while admin-only members panel is unavailable', async ({
    page,
    request,
  }) => {
    const handle = `iam-ordinary-${Date.now()}`
    const memberPass = 'Iam-Ordinary-Member-12!'

    const adminLogin = await request.post(`${apiBase}/api/v1/auth/login`, {
      data: { handle: 'admin', password: password! },
    })
    expect(adminLogin.ok()).toBeTruthy()
    const invite = await request.post(`${apiBase}/api/v1/orgs/demo/members`, {
      data: { handle, password: memberPass, role: 'member' },
    })
    expect(invite.ok()).toBeTruthy()

    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(memberPass)
    await page.getByTestId('login-handle-input').fill(handle)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, handle)

    await goAccountTokens(page)
    await expect(page.getByText(/Member admin requires org owner\/admin/i)).toBeVisible()

    const patName = `member-pat-${Date.now()}`
    await page.getByTestId('pat-name').fill(patName)
    await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes('/api/v1/tokens') && r.request().method() === 'POST' && r.ok(),
        { timeout: 15_000 },
      ),
      page.getByTestId('create-pat').click(),
    ])
    await expect(page.getByTestId('pat-minted-once')).toBeVisible()
    expect(await page.getByTestId('pat-minted-once').innerText()).toMatch(fullPatLike)
    await expect(page.getByTestId('pat-masked-list')).toContainText(patName)

    const list = await page.request.get(`${apiBase}/api/v1/tokens`)
    expect(list.ok()).toBeTruthy()
    const row = ((await list.json()) as { id: string; name: string }[]).find(
      (t) => t.name === patName,
    )
    expect(row?.id).toBeTruthy()
    await page.getByTestId(`revoke-pat-${row!.id}`).click()
    await expect(page.getByTestId('pat-masked-list')).not.toContainText(patName)
    await expect(page.getByTestId('app-alert')).toHaveCount(0)
  })
})
