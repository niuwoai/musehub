import { expect, test } from '@playwright/test'

import type { Page } from '@playwright/test'

import {
  STALE_DEPLOYMENT_BODY,
  STALE_DEPLOYMENT_ID,
  STALE_PLAN_PREFIX,
} from './fixtures/stale-deployment-view'
import { expectLoggedInAs } from './helpers/session'
import {
  createRepoViaUi,
  openRepoDelivery,
  repoNavButtonForSlug,
  repoNavButtons,
  waitForDemoRepoNavHydrated,
} from './helpers/repo'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

const HISTORY_DEPLOY_A = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
const HISTORY_DEPLOY_B = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
const ALL_TAB_ROW_ID = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd'

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
  await page.goto('/login')
  await fillLoginForm(page, 'admin', password!)
  await page.getByRole('button', { name: /login|登录/i }).click()
  await expectLoggedInAs(page, 'admin')
  await waitForDemoRepoNavHydrated(page)
  const { slug } = await createRepoViaUi(page)
  await openRepoDelivery(page, slug)
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
    const { repoEntry: repoBEntry } = await createRepoViaUi(page)
    await repoNavButtonForSlug(page, repoASlug).click()
    await expect(page.getByTestId(`deployment-pick-${repoAOnlyId}`)).toBeVisible({
      timeout: 15_000,
    })
    await repoBEntry.click()
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

    const staleListDone = page.waitForResponse(
      (r) => r.url().includes('/deployments?') && r.request().method() === 'GET',
    )
    await page.getByTestId('logout').click()
    release()
    await staleListDone

    await fillLoginForm(page, 'admin', password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)

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

  test('list refresh after delayed approval keeps active all-history tab', async ({ page }) => {
    const { gate, release } = delayedGate()

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
        body: JSON.stringify({
          items: [
            {
              id: ALL_TAB_ROW_ID,
              environment_slug: 'test',
              release_id: STALE_DEPLOYMENT_BODY.release_id,
              state: 'succeeded',
              plan_hash: null,
              approval_required: false,
              updated_at: '2026-06-01T00:00:00Z',
            },
          ],
          has_more: false,
        }),
      })
    })

    await page.route(`**/deployments/${STALE_DEPLOYMENT_ID}`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STALE_DEPLOYMENT_BODY),
      })
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
          state: 'running',
        }),
      })
    })

    await loginAndCreateRepo(page)
    await page.getByTestId(`deployment-pick-${STALE_DEPLOYMENT_ID}`).click()
    await expect(page.getByTestId('deployment-approval')).toBeVisible()

    const approvalReq = page.waitForRequest(
      (req) =>
        req.url().includes(`${STALE_DEPLOYMENT_ID}/approval`) && req.method() === 'POST',
    )
    const tabAllList = page.waitForResponse(
      (r) =>
        r.url().includes('/deployments?') &&
        r.request().method() === 'GET' &&
        !r.url().includes('state=awaiting_approval'),
    )
    await page.getByRole('button', { name: /approve deployment|批准部署/i }).click({ noWaitAfter: true })
    await approvalReq
    await page.getByTestId('deployment-view-all').click()
    await tabAllList
    const approvalDone = page.waitForResponse(
      (r) =>
        r.url().includes(`${STALE_DEPLOYMENT_ID}/approval`) &&
        r.request().method() === 'POST',
    )
    const postDecisionList = page.waitForResponse(
      (r) =>
        r.url().includes('/deployments?') &&
        r.request().method() === 'GET' &&
        !r.url().includes('state=awaiting_approval'),
    )
    release()
    await approvalDone
    await postDecisionList

    await expect(page.getByTestId('deployment-view-all')).toHaveAttribute('aria-pressed', 'true')
    await expect(page.getByTestId(`deployment-list-item-${ALL_TAB_ROW_ID}`)).toBeVisible()
    await expect(page.getByTestId(`deployment-row-state-${ALL_TAB_ROW_ID}`)).toHaveText(
      'succeeded',
    )
    await expect(page.getByTestId(`deployment-list-item-${STALE_DEPLOYMENT_ID}`)).toHaveCount(0)

    await page.unroute('**/deployments?*')
    await page.unroute(`**/deployments/${STALE_DEPLOYMENT_ID}`)
    await page.unroute(`**/deployments/${STALE_DEPLOYMENT_ID}/approval`)
  })
})
