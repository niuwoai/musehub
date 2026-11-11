import { expect, test } from '@playwright/test'

import type { Page } from '@playwright/test'

import { createRepoViaUi, openRepoDelivery, waitForDemoRepoNavHydrated } from './helpers/repo'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

const RUN_ID = '88888888-8888-4888-8888-888888888888'
const BUILD_JOB_ID = '99999999-9999-4999-8999-999999999999'
const MAIN_OID = 'b'.repeat(40)

async function fillLoginForm(page: Page, handle: string, pass: string) {
  const loginForm = page.locator('form').filter({ has: page.getByTestId('login-handle-input') })
  await loginForm.getByLabel(/password|密码/i).fill(pass)
  await page.getByTestId('login-handle-input').fill(handle)
}

test.describe('pipeline cancel console (route fixtures)', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('running pipeline cancel shows pending then canceled without stale poll overwrite', async ({
    page,
  }) => {
    let cancelPosts = 0
    let getCount = 0

    await page.route(/\/api\/v1\/orgs\/[^/]+\/repos\/[^/]+\/pull-requests$/, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([
          {
            id: 'pr-fixture-cancel',
            number: 2,
            title: 'merged',
            state: 'merged',
            source_branch: 'feature/y',
            target_branch: 'main',
            head_oid: MAIN_OID,
          },
        ]),
      })
    })

    await page.route('**/pipeline-runs', async (route) => {
      if (route.request().method() === 'POST') {
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({
            id: RUN_ID,
            source_oid: MAIN_OID,
            workflow_name: 'product-ci',
            state: 'queued',
            run_attempt_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
            error_code: null,
          }),
        })
        return
      }
      await route.continue()
    })

    let phase: 'active' | 'cancel_pending' | 'canceled' = 'active'
    let cancelPollGets = 0
    await page.route(`**/pipeline-runs/${RUN_ID}**`, async (route) => {
      const method = route.request().method()
      const url = route.request().url()
      if (method === 'POST' && url.includes('/cancel')) {
        cancelPosts += 1
        phase = 'cancel_pending'
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            id: RUN_ID,
            source_oid: MAIN_OID,
            workflow_name: 'product-ci',
            state: 'cancel_requested',
            run_attempt_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
            error_code: null,
          }),
        })
        return
      }
      if (method !== 'GET') {
        await route.continue()
        return
      }
      getCount += 1
      if (phase === 'active') {
        const runState = getCount < 2 ? 'queued' : 'running'
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            id: RUN_ID,
            source_oid: MAIN_OID,
            workflow_name: 'product-ci',
            state: runState,
            run_attempt_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
            error_code: null,
          }),
        })
        return
      }
      if (phase === 'cancel_pending') {
        cancelPollGets += 1
        const runState = cancelPollGets < 2 ? 'cancel_requested' : 'canceled'
        if (runState === 'canceled') {
          phase = 'canceled'
        }
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            id: RUN_ID,
            source_oid: MAIN_OID,
            workflow_name: 'product-ci',
            state: runState,
            run_attempt_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
            error_code: null,
          }),
        })
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: RUN_ID,
          source_oid: MAIN_OID,
          workflow_name: 'product-ci',
          state: 'canceled',
          run_attempt_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
          error_code: null,
        }),
      })
    })

    await page.route(`**/pipeline-runs/${RUN_ID}/jobs`, async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([
          {
            id: BUILD_JOB_ID,
            pipeline_run_id: RUN_ID,
            job_key: 'build',
            state: 'canceled',
            retry_allowed: false,
          },
        ]),
      })
    })

    await page.goto('/login')
    await fillLoginForm(page, 'admin', password!)
    await page.getByRole('button', { name: /sign in|login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoDelivery(page, slug)

    await expect(page.getByTestId('delivery-panel')).toContainText(/main @/, { timeout: 30_000 })
    await page.getByRole('button', { name: /run pipeline|流水线/i }).click()
    await expect(page.getByTestId('pipeline-state')).toContainText(/queued|running/i, {
      timeout: 30_000,
    })
    await expect(page.getByTestId('pipeline-cancel')).toBeVisible()

    await page.getByTestId('pipeline-cancel').click()
    expect(cancelPosts).toBe(1)
    await expect(page.getByTestId('pipeline-state')).toContainText(/cancel_requested|canceled/i, {
      timeout: 10_000,
    })
    await expect(page.getByTestId('pipeline-state')).toContainText(/canceled/i, { timeout: 30_000 })
    await expect(page.getByTestId('pipeline-cancel-honesty')).toBeVisible()
    await expect(page.getByTestId('pipeline-cancel-job-list')).toContainText('build')
    expect(getCount).toBeGreaterThan(0)
  })
})
