import { expect, test } from '@playwright/test'

import { createRepoViaUi, openRepoDelivery, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin, waitForSessionAuthSettled } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const MAIN_OID = 'b'.repeat(40)
const RUN_ID = '88888888-8888-4888-8888-888888888888'
const REL_ID = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'

test.describe('REL-01 publish release console', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('publish shows published row after refresh; publish-only does not start deployment', async ({
    page,
  }) => {
    type ReleaseRow = {
      id: string
      version: string
      tag_name: string
      commit_oid: string
      pipeline_run_id: string
      state: string
      manifest_sha256?: string
    }
    const releaseStore: ReleaseRow[] = []
    let createSeen = 0
    let publishSeen = 0
    let deployPosts = 0

    page.on('request', (req) => {
      if (req.method() === 'POST' && req.url().includes('/deployments')) {
        deployPosts += 1
      }
    })

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
            id: 'pr-rel-1',
            number: 1,
            title: 'merged',
            state: 'merged',
            source_branch: 'feature/rel',
            target_branch: 'main',
            head_oid: MAIN_OID,
          },
        ]),
      })
    })

    await page.route(`**/pipeline-runs/${RUN_ID}`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: RUN_ID,
          source_oid: MAIN_OID,
          workflow_name: 'release',
          state: 'succeeded',
          run_attempt_id: '99999999-9999-4999-8999-999999999999',
          error_code: null,
        }),
      })
    })

    await page.route(/\/releases$/, async (route) => {
      const method = route.request().method()
      if (method === 'POST') {
        createSeen += 1
        const row: ReleaseRow = {
          id: REL_ID,
          version: '0.0.1',
          tag_name: 'v0.0.1',
          commit_oid: MAIN_OID,
          pipeline_run_id: RUN_ID,
          state: 'draft',
          manifest_sha256: 'c'.repeat(64),
        }
        releaseStore.length = 0
        releaseStore.push(row)
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify(row),
        })
        return
      }
      if (method === 'GET') {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(releaseStore),
        })
        return
      }
      await route.continue()
    })

    await page.route(/\/releases\/[^/]+\/publish$/, async (route) => {
      if (route.request().method() !== 'POST') {
        await route.continue()
        return
      }
      await new Promise((r) => setTimeout(r, 400))
      publishSeen += 1
      const published: ReleaseRow = {
        id: REL_ID,
        version: '0.0.1',
        tag_name: 'v0.0.1',
        commit_oid: MAIN_OID,
        pipeline_run_id: RUN_ID,
        state: 'published',
        manifest_sha256: 'c'.repeat(64),
      }
      releaseStore.length = 0
      releaseStore.push(published)
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(published),
      })
    })

    await loginAsAdmin(page, password!)
    await waitForSessionAuthSettled(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoDelivery(page, slug)

    await page.route('**/pipeline-runs', async (route) => {
      if (route.request().method() === 'POST') {
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({
            id: RUN_ID,
            source_oid: MAIN_OID,
            workflow_name: 'release',
            state: 'succeeded',
            run_attempt_id: '99999999-9999-4999-8999-999999999999',
          }),
        })
        return
      }
      await route.continue()
    })

    await page.getByRole('button', { name: /run pipeline|运行流水线/i }).click()
    await expect(page.getByTestId('pipeline-state')).toContainText(/succeeded/i, { timeout: 15_000 })
    const publishBtn = page.getByTestId('publish-pipeline-release')
    await expect(publishBtn).toBeVisible()
    await expect(page.getByTestId('delivery-releases-empty')).toBeVisible()

    const publishClick = publishBtn.click()
    await expect(page.getByTestId('release-and-deploy-mock')).toBeDisabled()
    await expect(publishBtn).toBeDisabled()
    await publishClick

    await expect
      .poll(() => createSeen, { timeout: 15_000 })
      .toBe(1)
    await expect
      .poll(() => publishSeen, { timeout: 15_000 })
      .toBe(1)
    await expect
      .poll(() => deployPosts, { timeout: 5_000 })
      .toBe(0)

    await expect(page.getByTestId('release-row-v0.0.1')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('release-row-v0.0.1')).toContainText(/published/i)

    await publishBtn.click()
    await expect
      .poll(() => createSeen, { timeout: 5_000 })
      .toBe(1)
    await expect
      .poll(() => publishSeen, { timeout: 5_000 })
      .toBe(1)
  })

  test('publish failure surfaces error and leaves list empty', async ({ page }) => {
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
            id: 'pr-rel-2',
            number: 1,
            title: 'merged',
            state: 'merged',
            source_branch: 'feature/rel',
            target_branch: 'main',
            head_oid: MAIN_OID,
          },
        ]),
      })
    })

    await page.route(`**/pipeline-runs/${RUN_ID}`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: RUN_ID,
          source_oid: MAIN_OID,
          workflow_name: 'release',
          state: 'succeeded',
          run_attempt_id: '99999999-9999-4999-8999-999999999999',
          error_code: null,
        }),
      })
    })

    await page.route(/\/releases$/, async (route) => {
      if (route.request().method() === 'GET') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' })
        return
      }
      if (route.request().method() === 'POST') {
        await route.fulfill({ status: 403, body: 'forbidden' })
        return
      }
      await route.continue()
    })

    await loginAsAdmin(page, password!)
    await waitForSessionAuthSettled(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoDelivery(page, slug)

    await page.route('**/pipeline-runs', async (route) => {
      if (route.request().method() === 'POST') {
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({
            id: RUN_ID,
            source_oid: MAIN_OID,
            workflow_name: 'release',
            state: 'succeeded',
            run_attempt_id: '99999999-9999-4999-8999-999999999999',
          }),
        })
        return
      }
      await route.continue()
    })

    await page.getByRole('button', { name: /run pipeline|运行流水线/i }).click()
    await expect(page.getByTestId('pipeline-state')).toContainText(/succeeded/i, { timeout: 15_000 })
    await page.getByTestId('publish-pipeline-release').click()
    await expect(page.getByTestId('delivery-workspace-error')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('delivery-releases-empty')).toBeVisible()
    await expect(page.getByTestId('release-and-deploy-mock')).toBeEnabled()
  })

  /**
   * PR list + pipeline start/poll are mocked; release create/publish hit the real API.
   * Pipeline `succeeded` row is not produced by a real runner — only UI step is synthetic.
   */
  test('release APIs unmocked after mocked pipeline succeeded (no real runner)', async ({
    page,
  }) => {
    test.slow()

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
            id: 'pr-rel-real',
            number: 1,
            title: 'merged',
            state: 'merged',
            source_branch: 'feature/rel',
            target_branch: 'main',
            head_oid: MAIN_OID,
          },
        ]),
      })
    })

    await page.route(`**/pipeline-runs/${RUN_ID}`, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: RUN_ID,
          source_oid: MAIN_OID,
          workflow_name: 'release',
          state: 'succeeded',
          run_attempt_id: '99999999-9999-4999-8999-999999999999',
          error_code: null,
        }),
      })
    })

    await loginAsAdmin(page, password!)
    await waitForSessionAuthSettled(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoDelivery(page, slug)

    await page.route('**/pipeline-runs', async (route) => {
      if (route.request().method() === 'POST') {
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({
            id: RUN_ID,
            source_oid: MAIN_OID,
            workflow_name: 'release',
            state: 'succeeded',
            run_attempt_id: '99999999-9999-4999-8999-999999999999',
          }),
        })
        return
      }
      await route.continue()
    })

    await page.getByRole('button', { name: /run pipeline|运行流水线/i }).click()
    await expect(page.getByTestId('pipeline-state')).toContainText(/succeeded/i, { timeout: 15_000 })

    const releasePost = page.waitForResponse(
      (r) => r.request().method() === 'POST' && /\/releases(\/|$)/.test(r.url()),
      { timeout: 15_000 },
    )
    await page.getByTestId('publish-pipeline-release').click()
    const apiResp = await releasePost
    expect(apiResp.status()).toBeGreaterThanOrEqual(400)
    await expect(page.getByTestId('delivery-workspace-error')).toBeVisible()
    await expect(page.getByTestId('delivery-releases-empty')).toBeVisible()
  })
})
