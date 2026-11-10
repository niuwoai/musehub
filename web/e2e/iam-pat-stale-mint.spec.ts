import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'
const fullPatLike = /mhp_[a-zA-Z0-9]{24,}/

async function gateRoute(page: import('@playwright/test').Page) {
  let release: (() => void) | undefined
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  await page.route('**/api/v1/tokens', async (route) => {
    if (route.request().method() === 'POST') {
      await gate
      await route.continue()
      return
    }
    await route.continue()
  })
  return { release: () => release!() }
}

test.describe('PAT mint session epoch guard', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('delayed PAT create does not reveal secret after logout and account switch', async ({
    page,
    request,
  }) => {
    const memberHandle = `iam-stale-${Date.now()}`
    const memberPass = 'Iam-Stale-Switch-12!'

    const adminLogin = await request.post(`${apiBase}/api/v1/auth/login`, {
      data: { handle: 'admin', password: password! },
    })
    expect(adminLogin.ok()).toBeTruthy()
    const invite = await request.post(`${apiBase}/api/v1/orgs/demo/members`, {
      data: { handle: memberHandle, password: memberPass, role: 'member' },
    })
    expect(invite.ok()).toBeTruthy()

    const adminGate = await gateRoute(page)

    await page.goto('/')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')

    await page.getByTestId('pat-name').fill(`stale-admin-${Date.now()}`)
    void page.getByTestId('create-pat').click()
    await page.getByTestId('logout').click()
    await expect(page.getByTestId('session-handle')).toHaveCount(0)
    adminGate.release()
    await page.waitForTimeout(800)
    expect(await page.locator('body').innerText()).not.toMatch(fullPatLike)

    await page.unroute('**/api/v1/tokens')
    const memberGate = await gateRoute(page)

    await page.getByTestId('login-handle-input').fill(memberHandle)
    await page.getByLabel(/password|密码/i).fill(memberPass)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, memberHandle)

    await page.getByTestId('pat-name').fill(`stale-member-${Date.now()}`)
    void page.getByTestId('create-pat').click()
    await page.getByTestId('logout').click()
    await expect(page.getByTestId('session-handle')).toHaveCount(0)
    memberGate.release()
    await page.waitForTimeout(800)
    expect(await page.locator('body').innerText()).not.toMatch(fullPatLike)
  })
})
