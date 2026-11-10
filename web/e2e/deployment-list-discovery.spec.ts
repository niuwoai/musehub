import { expect, test } from '@playwright/test'

import type { Page } from '@playwright/test'

import {
  STALE_DEPLOYMENT_BODY,
  STALE_DEPLOYMENT_ID,
  STALE_PLAN_PREFIX,
} from './fixtures/stale-deployment-view'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

function repoNavButtons(page: Page) {
  return page.locator('section > ul').first().getByRole('button')
}

async function fillLoginForm(page: Page, handle: string, pass: string) {
  const loginForm = page.locator('form').filter({ has: page.getByTestId('login-handle-input') })
  await loginForm.getByLabel(/password|密码/i).fill(pass)
  await page.getByTestId('login-handle-input').fill(handle)
}

function delayedGate() {
  let release: () => void = () => {}
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  return { gate, release: () => release() }
}

test.describe('deployment list discovery', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('pending list fixture opens approval UI without manual ID', async ({ page }) => {
    await page.route('**/deployments?*', async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      const url = route.request().url()
      if (!url.includes('state=awaiting_approval')) {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          items: [
            {
              id: STALE_DEPLOYMENT_ID,
              environment_slug: 'production',
              release_id: STALE_DEPLOYMENT_BODY.release_id,
              state: 'awaiting_approval',
              plan_hash: STALE_DEPLOYMENT_BODY.plan_hash,
              approval_required: true,
              updated_at: '2026-01-01T00:00:00Z',
            },
          ],
          has_more: false,
        }),
      })
    })

    await page.route(`**/deployments/${STALE_DEPLOYMENT_ID}`, async (route) => {
      if (route.request().method() === 'GET') {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(STALE_DEPLOYMENT_BODY),
        })
        return
      }
      await route.continue()
    })

    await page.goto('/')
    await fillLoginForm(page, 'admin', password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')

    const before = await repoNavButtons(page).count()
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await expect(repoNavButtons(page)).toHaveCount(before + 1, { timeout: 15_000 })
    await repoNavButtons(page).last().click()

    await expect(page.getByTestId('deployment-list')).toBeVisible({ timeout: 15_000 })
    await page.getByTestId(`deployment-pick-${STALE_DEPLOYMENT_ID}`).click()
    await expect(page.getByTestId('deployment-approval')).toBeVisible()
    await expect(page.getByTestId('deployment-plan-hash')).toContainText(STALE_PLAN_PREFIX)

    await page.unroute('**/deployments?*')
    await page.unroute(`**/deployments/${STALE_DEPLOYMENT_ID}`)
  })

  test('stale delayed list refresh does not apply after repo switch', async ({ page }) => {
    const { gate, release } = delayedGate()

    await page.route('**/deployments?*', async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await gate
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          items: [
            {
              id: STALE_DEPLOYMENT_ID,
              environment_slug: 'production',
              release_id: STALE_DEPLOYMENT_BODY.release_id,
              state: 'awaiting_approval',
              plan_hash: STALE_DEPLOYMENT_BODY.plan_hash,
              approval_required: true,
              updated_at: '2026-01-01T00:00:00Z',
            },
          ],
          has_more: false,
        }),
      })
    })

    await page.goto('/')
    await fillLoginForm(page, 'admin', password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')

    const before = await repoNavButtons(page).count()
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await expect(repoNavButtons(page)).toHaveCount(before + 1, { timeout: 15_000 })
    const repoA = repoNavButtons(page).nth(before)
    await repoA.click()

    const listReq = page.waitForRequest(
      (req) => req.url().includes('/deployments?') && req.method() === 'GET',
    )
    await page.getByTestId('refresh-deployments').click()
    await listReq

    const beforeB = await repoNavButtons(page).count()
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await expect(repoNavButtons(page)).toHaveCount(beforeB + 1, { timeout: 15_000 })
    await repoNavButtons(page).last().click()

    const listDone = page.waitForResponse(
      (r) => r.url().includes('/deployments?') && r.request().method() === 'GET',
    )
    release()
    await listDone

    await expect(page.getByTestId(`deployment-pick-${STALE_DEPLOYMENT_ID}`)).toHaveCount(0)
    const body = await page.locator('body').innerText()
    expect(body).not.toContain(STALE_PLAN_PREFIX)

    await page.unroute('**/deployments?*')
  })
})
