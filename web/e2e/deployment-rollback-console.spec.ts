import { expect, test } from '@playwright/test'

import type { Page } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

const FAILED_DEPLOY_ID = 'f1111111-1111-4111-8111-111111111111'
const GOOD_DEPLOY_ID = 'g2222222-2222-4222-8222-222222222222'
const ROLLBACK_RESULT_ID = 'r3333333-3333-4333-8333-333333333333'
const RELEASE_ID = '22222222-2222-4222-8222-222222222222'

function repoNavButtons(page: Page) {
  return page.locator('section > ul').first().getByRole('button')
}

async function fillLoginForm(page: Page, handle: string, pass: string) {
  const loginForm = page.locator('form').filter({ has: page.getByTestId('login-handle-input') })
  await loginForm.getByLabel(/password|密码/i).fill(pass)
  await page.getByTestId('login-handle-input').fill(handle)
}

test.describe('deployment rollback console', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('failed deployment shows diagnosis and rollback to known good', async ({ page }) => {
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
              id: FAILED_DEPLOY_ID,
              environment_slug: 'test',
              release_id: RELEASE_ID,
              state: 'failed',
              plan_hash: null,
              approval_required: false,
              updated_at: '2026-03-02T00:00:00Z',
            },
            {
              id: GOOD_DEPLOY_ID,
              environment_slug: 'test',
              release_id: RELEASE_ID,
              state: 'succeeded',
              plan_hash: null,
              approval_required: false,
              updated_at: '2026-03-01T00:00:00Z',
            },
          ],
          has_more: false,
        }),
      })
    })

    await page.route(`**/deployments/${FAILED_DEPLOY_ID}`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: FAILED_DEPLOY_ID,
          environment_slug: 'test',
          release_id: RELEASE_ID,
          state: 'failed',
          adapter_kind: 'compose-mock',
          approval_required: false,
          health_summary: {
            status: 'unhealthy',
            error_code: 'deploy_failed',
            detail: 'mock health check failed',
          },
        }),
      })
    })

    await page.route(`**/trace/deployments/${FAILED_DEPLOY_ID}`, async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          deployment_id: FAILED_DEPLOY_ID,
          release_id: RELEASE_ID,
          commit_oid: 'a'.repeat(40),
          deployment_adapter: 'compose-mock',
          adapter_is_mock: true,
          gaps: ['mock-gap'],
          artifacts: [],
        }),
      })
    })

    await page.route(`**/trace/deployments/${ROLLBACK_RESULT_ID}`, async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          deployment_id: ROLLBACK_RESULT_ID,
          release_id: RELEASE_ID,
          commit_oid: 'b'.repeat(40),
          deployment_adapter: 'compose-mock',
          adapter_is_mock: true,
          gaps: [],
          artifacts: [],
        }),
      })
    })

    await page.route('**/deployments/rollback', async (route) => {
      if (route.request().method() !== 'POST') {
        await route.continue()
        return
      }
      const body = route.request().postDataJSON() as { target_deployment_id?: string }
      expect(body.target_deployment_id).toBe(GOOD_DEPLOY_ID)
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: ROLLBACK_RESULT_ID,
          environment_slug: 'test',
          release_id: RELEASE_ID,
          state: 'succeeded',
          adapter_kind: 'compose-mock',
          approval_required: false,
          health_summary: { rollback: true, status: 'healthy' },
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
    await repoNavButtons(page).last().click()

    await page.getByTestId('deployment-view-all').click()
    const detailGet = page.waitForResponse(
      (r) => r.url().includes(FAILED_DEPLOY_ID) && r.request().method() === 'GET',
    )
    await page.getByTestId(`deployment-pick-${FAILED_DEPLOY_ID}`).click()
    await detailGet
    await expect(page.getByTestId('deployment-failure-panel')).toBeVisible()
    await expect(page.getByTestId('deployment-failure-code')).toHaveText('deploy_failed')

    const rollbackReq = page.waitForRequest(
      (req) => req.url().includes('/deployments/rollback') && req.method() === 'POST',
    )
    await page.getByTestId('deployment-rollback-known-good').click({ noWaitAfter: true })
    await rollbackReq

    await expect(page.getByTestId('deployment-failure-panel')).toHaveCount(0)
    await expect(page.getByTestId('deployment-lookup-id')).toHaveValue(ROLLBACK_RESULT_ID)

    await page.unroute('**/deployments?*')
    await page.unroute(`**/deployments/${FAILED_DEPLOY_ID}`)
    await page.unroute('**/deployments/rollback')
  })
})
