import { expect, test } from '@playwright/test'

import type { Page } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

const RUN_ID = '55555555-5555-4555-8555-555555555555'
const BUILD_JOB_ID = '66666666-6666-4666-8666-666666666666'
const MAIN_OID = 'a'.repeat(40)

async function fillLoginForm(page: Page, handle: string, pass: string) {
  const loginForm = page.locator('form').filter({ has: page.getByTestId('login-handle-input') })
  await loginForm.getByLabel(/password|密码/i).fill(pass)
  await page.getByTestId('login-handle-input').fill(handle)
}

test.describe('pipeline job retry console (route fixtures)', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('failed pipeline shows diagnosis and retries build job via session API', async ({
    page,
  }) => {
    let retryCount = 0

    await page.route('**/pull-requests?*', async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([
          {
            id: 'pr-fixture-1',
            number: 1,
            title: 'merged',
            state: 'merged',
            source_branch: 'feature/x',
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
            run_attempt_id: '77777777-7777-4777-8777-777777777777',
            error_code: null,
          }),
        })
        return
      }
      await route.continue()
    })

    let pollPhase: 'fail' | 'ok' = 'fail'
    await page.route(`**/pipeline-runs/${RUN_ID}`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      if (pollPhase === 'fail') {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            id: RUN_ID,
            source_oid: MAIN_OID,
            workflow_name: 'product-ci',
            state: 'failed',
            run_attempt_id: '77777777-7777-4777-8777-777777777777',
            error_code: 'build_failed',
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
          state: 'succeeded',
          run_attempt_id: '77777777-7777-4777-8777-777777777777',
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
            state: 'failed',
            retry_allowed: true,
          },
        ]),
      })
    })

    await page.route(`**/pipeline-jobs/${BUILD_JOB_ID}/retry`, async (route) => {
      retryCount += 1
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: BUILD_JOB_ID,
          pipeline_run_id: RUN_ID,
          job_key: 'build',
          state: 'queued',
          retry_allowed: false,
        }),
      })
      pollPhase = 'ok'
    })

    await page.goto('/')
    await fillLoginForm(page, 'admin', password!)
    await page.getByRole('button', { name: /sign in|登录/i }).click()
    await expectLoggedInAs(page, 'admin')

    const slug = `pipe-retry-ui-${Date.now().toString(36)}`
    await page.getByPlaceholder(/slug|仓库标识/i).fill(slug)
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await expect(page.getByRole('button', { name: slug })).toBeVisible({ timeout: 30_000 })
    await page.getByRole('button', { name: slug }).click()

    await expect(page.getByTestId('delivery-panel')).toContainText(/main @/, { timeout: 30_000 })
    await page.getByRole('button', { name: /run pipeline|流水线/i }).click()
    await expect(page.getByTestId('pipeline-state')).toContainText(/failed/i, { timeout: 30_000 })
    await expect(page.getByTestId('pipeline-failure-panel')).toBeVisible()
    await expect(page.getByTestId('pipeline-failure-code')).toContainText('build_failed')
    await expect(page.getByTestId('pipeline-job-list')).toContainText('build')

    await page.getByTestId('pipeline-retry-build').click()
    await expect(page.getByTestId('pipeline-state')).toContainText(/succeeded/i, {
      timeout: 30_000,
    })
    expect(retryCount).toBe(1)
  })
})
