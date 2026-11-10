import { expect, test } from '@playwright/test'

import type { Page } from '@playwright/test'

import {
  STALE_DEPLOYMENT_BODY,
  STALE_DEPLOYMENT_ID,
  STALE_PLAN_PREFIX,
  STALE_TRACE_BODY,
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

async function loginAsAdmin(page: Page) {
  await page.goto('/')
  await fillLoginForm(page, 'admin', password!)
  await page.getByRole('button', { name: /login|登录/i }).click()
  await expectLoggedInAs(page, 'admin')
}

async function createEmptyRepo(page: Page) {
  const before = await repoNavButtons(page).count()
  await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
  await expect(repoNavButtons(page)).toHaveCount(before + 1, { timeout: 15_000 })
  const entry = repoNavButtons(page).last()
  const slug = (await entry.textContent())?.trim() ?? ''
  await entry.click()
  return slug
}

function delayedGate() {
  let release: () => void = () => {}
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  return { gate, release: () => release() }
}

test.describe('deployment view session/repo epoch', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('stale deployment GET after logout/login does not render fixture deployment', async ({
    page,
  }) => {
    const { gate, release } = delayedGate()

    await page.route(`**/deployments/${STALE_DEPLOYMENT_ID}`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await gate
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STALE_DEPLOYMENT_BODY),
      })
    })

    await loginAsAdmin(page)
    await createEmptyRepo(page)
    await page.getByTestId('deployment-lookup-id').fill(STALE_DEPLOYMENT_ID)
    const loadReq = page.waitForRequest(
      (req) => req.url().includes(STALE_DEPLOYMENT_ID) && req.method() === 'GET',
    )
    await page.getByTestId('load-deployment').click()
    await loadReq

    await page.getByTestId('logout').click()
    release()
    await page.waitForTimeout(400)

    await fillLoginForm(page, 'admin', password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await expect(page.getByTestId('deployment-approval')).toHaveCount(0)
    const body = await page.locator('body').innerText()
    expect(body).not.toContain(STALE_PLAN_PREFIX)

    await page.unroute(`**/deployments/${STALE_DEPLOYMENT_ID}`)
  })

  test('stale deployment GET after repo switch does not write into new repo view', async ({
    page,
  }) => {
    const { gate, release } = delayedGate()
    await page.route(`**/deployments/${STALE_DEPLOYMENT_ID}`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await gate
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STALE_DEPLOYMENT_BODY),
      })
    })

    await loginAsAdmin(page)
    const repoA = await createEmptyRepo(page)
    await page.getByTestId('deployment-lookup-id').fill(STALE_DEPLOYMENT_ID)
    const loadReq = page.waitForRequest(
      (req) => req.url().includes(STALE_DEPLOYMENT_ID) && req.method() === 'GET',
    )
    await page.getByTestId('load-deployment').click()
    await loadReq

    const beforeB = await repoNavButtons(page).count()
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await expect(repoNavButtons(page)).toHaveCount(beforeB + 1, { timeout: 15_000 })
    const repoB = (await repoNavButtons(page).last().textContent())?.trim() ?? ''
    await repoNavButtons(page).last().click()
    expect(repoB).not.toBe(repoA)

    release()
    await page.waitForTimeout(250)

    await expect(page.getByTestId('deployment-approval')).toHaveCount(0)
    const body = await page.locator('body').innerText()
    expect(body).not.toContain(STALE_PLAN_PREFIX)

    await page.unroute(`**/deployments/${STALE_DEPLOYMENT_ID}`)
  })

  test('stale approve refresh/trace after logout does not repopulate panel', async ({ page }) => {
    const { gate, release } = delayedGate()
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

    await page.route(`**/deployments/${STALE_DEPLOYMENT_ID}/approval`, async (route) => {
      if (route.request().method() !== 'POST') {
        await route.continue()
        return
      }
      await gate
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          deployment_id: STALE_DEPLOYMENT_ID,
          decision: 'approve',
          state: 'succeeded',
          plan_hash: STALE_DEPLOYMENT_BODY.plan_hash,
        }),
      })
    })

    await page.route(`**/trace/deployments/${STALE_DEPLOYMENT_ID}`, async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STALE_TRACE_BODY),
      })
    })

    await loginAsAdmin(page)
    await createEmptyRepo(page)
    await page.getByTestId('deployment-lookup-id').fill(STALE_DEPLOYMENT_ID)
    await page.getByTestId('load-deployment').click()
    await expect(page.getByTestId('deployment-approval')).toBeVisible()

    const approveReq = page.waitForRequest(
      (req) => req.url().includes('/approval') && req.method() === 'POST',
    )
    void page.getByTestId('approve-deployment').click()
    await approveReq
    await page.getByTestId('logout').click()
    await expect(page.getByTestId('deployment-approval')).toHaveCount(0)
    release()
    await page.waitForTimeout(400)

    await fillLoginForm(page, 'admin', password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await expect(page.getByTestId('deployment-approval')).toHaveCount(0)
    await expect(page.getByTestId('delivery-trace')).toHaveCount(0)
    const body = await page.locator('body').innerText()
    expect(body).not.toContain(STALE_PLAN_PREFIX)

    await page.unroute(`**/deployments/${STALE_DEPLOYMENT_ID}`)
    await page.unroute(`**/deployments/${STALE_DEPLOYMENT_ID}/approval`)
    await page.unroute(`**/trace/deployments/${STALE_DEPLOYMENT_ID}`)
  })
})
