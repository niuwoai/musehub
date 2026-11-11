import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'
import { STALE_MINT_FIXTURE_BODY, STALE_MINT_FIXTURE_TOKEN } from './fixtures/stale-mint-pat'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

async function installPausedPostPatRoute(page: import('@playwright/test').Page) {
  let releaseGate: (() => void) | undefined
  const gate = new Promise<void>((resolve) => {
    releaseGate = resolve
  })
  let postSeenResolve: () => void = () => {}
  const postSeen = new Promise<void>((resolve) => {
    postSeenResolve = resolve
  })

  await page.route('**/api/v1/tokens', async (route) => {
    if (route.request().method() === 'POST') {
      postSeenResolve()
      await gate
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STALE_MINT_FIXTURE_BODY),
      })
      return
    }
    await route.continue()
  })

  return {
    postSeen,
    release: () => releaseGate!(),
    unroute: () => page.unroute('**/api/v1/tokens'),
  }
}

test.describe('PAT mint session epoch guard', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('stale delayed 200 fixture never renders; next session Create PAT enabled', async ({
    page,
  }) => {
    const paused = await installPausedPostPatRoute(page)

    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')

    const responseDone = page.waitForResponse(
      (r) => r.url().includes('/api/v1/tokens') && r.request().method() === 'POST',
    )

    await page.getByTestId('pat-name').fill('stale-fixture-admin')
    await page.getByTestId('create-pat').click()
    await paused.postSeen

    await page.getByTestId('logout').click()
    await expect(page.getByTestId('session-handle')).toHaveCount(0)

    paused.release()
    const staleResponse = await responseDone
    expect(staleResponse.ok()).toBeTruthy()
    const json = (await staleResponse.json()) as { token: string }
    expect(json.token).toBe(STALE_MINT_FIXTURE_TOKEN)

    await expect(page.getByTestId('pat-minted-once')).toHaveCount(0)
    const loggedOutBody = await page.locator('body').innerText()
    expect(loggedOutBody).not.toContain(STALE_MINT_FIXTURE_TOKEN)

    await paused.unroute()

    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await expect(page.getByTestId('create-pat')).toBeEnabled()

    const patName = `after-stale-busy-${Date.now()}`
    await page.getByTestId('pat-name').fill(patName)
    const realCreate = page.waitForResponse(
      (r) => r.url().includes('/api/v1/tokens') && r.request().method() === 'POST' && r.ok(),
    )
    await page.getByTestId('create-pat').click()
    await realCreate
    await expect(page.getByTestId('pat-minted-once')).toBeVisible()
    expect(await page.getByTestId('pat-minted-once').innerText()).not.toContain(
      STALE_MINT_FIXTURE_TOKEN,
    )
    await expect(page.getByTestId('pat-masked-list')).toContainText(patName)
  })
})
