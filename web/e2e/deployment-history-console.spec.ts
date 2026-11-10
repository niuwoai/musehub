import { expect, test } from '@playwright/test'

import type { Page } from '@playwright/test'

import {
  STALE_DEPLOYMENT_BODY,
  STALE_DEPLOYMENT_ID,
  STALE_PLAN_PREFIX,
} from './fixtures/stale-deployment-view'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

const HISTORY_DEPLOY_A = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
const HISTORY_DEPLOY_B = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'

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

async function loginAndCreateRepo(page: Page) {
  await page.goto('/')
  await fillLoginForm(page, 'admin', password!)
  await page.getByRole('button', { name: /login|登录/i }).click()
  await expectLoggedInAs(page, 'admin')
  const before = await repoNavButtons(page).count()
  await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
  await expect(repoNavButtons(page)).toHaveCount(before + 1, { timeout: 15_000 })
  const slugBtn = repoNavButtons(page).last()
  const slug = (await slugBtn.textContent())?.trim() ?? ''
  await slugBtn.click()
  return slug
}

test.describe('deployment history console', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('all history view lists mixed states without pending filter', async ({ page }) => {
    await page.route('**/deployments?*', async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      const url = route.request().url()
      if (url.includes('state=awaiting_approval')) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ items: [], has_more: false }),
        })
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          items: [
            {
              id: HISTORY_DEPLOY_A,
              environment_slug: 'test',
              release_id: STALE_DEPLOYMENT_BODY.release_id,
              state: 'succeeded',
              plan_hash: null,
              approval_required: false,
              updated_at: '2026-02-01T12:00:00Z',
            },
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

    await loginAndCreateRepo(page)
    await page.getByTestId('deployment-view-all').click()
    await expect(page.getByTestId(`deployment-list-item-${HISTORY_DEPLOY_A}`)).toBeVisible({
      timeout: 15_000,
    })
    await expect(page.getByTestId(`deployment-row-state-${HISTORY_DEPLOY_A}`)).toHaveText(
      'succeeded',
    )
    await expect(page.getByTestId(`deployment-list-item-${STALE_DEPLOYMENT_ID}`)).toBeVisible()

    await page.unroute('**/deployments?*')
  })

  test('load more appends second page in all history', async ({ page }) => {
    let listCall = 0
    await page.route('**/deployments?*', async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      const url = route.request().url()
      if (url.includes('state=awaiting_approval')) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ items: [], has_more: false }),
        })
        return
      }
      const offset = new URL(url).searchParams.get('offset') ?? '0'
      if (offset === '0') {
        listCall += 1
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            items: [
              {
                id: HISTORY_DEPLOY_A,
                environment_slug: 'test',
                release_id: STALE_DEPLOYMENT_BODY.release_id,
                state: 'succeeded',
                plan_hash: null,
                approval_required: false,
                updated_at: '2026-03-01T00:00:00Z',
              },
            ],
            has_more: true,
          }),
        })
        return
      }
      listCall += 1
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          items: [
            {
              id: HISTORY_DEPLOY_B,
              environment_slug: 'production',
              release_id: STALE_DEPLOYMENT_BODY.release_id,
              state: 'failed',
              plan_hash: null,
              approval_required: false,
              updated_at: '2026-02-01T00:00:00Z',
            },
          ],
          has_more: false,
        }),
      })
    })

    await loginAndCreateRepo(page)
    await page.getByTestId('deployment-view-all').click()
    await expect(page.getByTestId(`deployment-list-item-${HISTORY_DEPLOY_A}`)).toBeVisible({
      timeout: 15_000,
    })
    const page2 = page.waitForResponse(
      (r) => r.url().includes('/deployments?') && r.url().includes('offset=1'),
    )
    await page.getByTestId('deployment-load-more').click()
    await page2
    await expect(page.getByTestId(`deployment-list-item-${HISTORY_DEPLOY_B}`)).toBeVisible()
    expect(listCall).toBeGreaterThanOrEqual(2)

    await page.unroute('**/deployments?*')
  })

  test('second repo does not show first repo list rows', async ({ page }) => {
    const repoAOnlyId = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc'
    let repoASlug = ''

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
      if (repoASlug && url.includes(`/repos/${repoASlug}/`)) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            items: [
              {
                id: repoAOnlyId,
                environment_slug: 'production',
                release_id: STALE_DEPLOYMENT_BODY.release_id,
                state: 'awaiting_approval',
                plan_hash: STALE_PLAN_PREFIX + '0'.repeat(52),
                approval_required: true,
                updated_at: '2026-01-02T00:00:00Z',
              },
            ],
            has_more: false,
          }),
        })
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ items: [], has_more: false }),
      })
    })

    repoASlug = await loginAndCreateRepo(page)
    const repoCount = await repoNavButtons(page).count()
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await expect(repoNavButtons(page)).toHaveCount(repoCount + 1, { timeout: 15_000 })
    await repoNavButtons(page).filter({ hasText: repoASlug }).click()
    await expect(page.getByTestId(`deployment-pick-${repoAOnlyId}`)).toBeVisible({
      timeout: 15_000,
    })
    await repoNavButtons(page).last().click()
    await expect(page.getByTestId(`deployment-pick-${repoAOnlyId}`)).toHaveCount(0, {
      timeout: 10_000,
    })

    await page.unroute('**/deployments?*')
  })

  test('stale delayed list does not apply after logout login and repo reselect', async ({
    page,
  }) => {
    const { gate, release } = delayedGate()
    let listGets = 0
    let savedSlug = ''

    await page.route('**/deployments?*', async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      listGets += 1
      if (listGets === 2) {
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
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ items: [], has_more: false }),
      })
    })

    savedSlug = await loginAndCreateRepo(page)
    await expect(page.getByTestId('deployment-list-empty')).toBeVisible({ timeout: 15_000 })
    const listReq = page.waitForRequest(
      (req) => req.url().includes('/deployments?') && req.method() === 'GET',
    )
    await page.getByTestId('refresh-deployments').click({ noWaitAfter: true })
    await listReq

    await page.getByTestId('logout').click()
    const staleListDone = page.waitForResponse(
      (r) => r.url().includes('/deployments?') && r.request().method() === 'GET',
    )
    release()
    await staleListDone

    await fillLoginForm(page, 'admin', password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')

    const reselect = page.getByRole('button', { name: savedSlug, exact: true })
    await expect(reselect).toBeVisible({ timeout: 15_000 })
    const remountList = page.waitForResponse(
      (r) =>
        r.url().includes('/deployments?') &&
        r.request().method() === 'GET' &&
        r.url().includes(savedSlug),
    )
    const otherRepo = repoNavButtons(page).filter({ hasNotText: savedSlug }).first()
    if ((await otherRepo.count()) > 0) {
      await otherRepo.click()
    }
    await reselect.click()
    await remountList
    await expect(page.getByTestId('deployment-list-empty')).toBeVisible({ timeout: 15_000 })

    await expect(page.getByTestId(`deployment-pick-${STALE_DEPLOYMENT_ID}`)).toHaveCount(0, {
      timeout: 10_000,
    })
    const body = await page.locator('body').innerText()
    expect(body).not.toContain(STALE_PLAN_PREFIX)

    await page.unroute('**/deployments?*')
  })
})
