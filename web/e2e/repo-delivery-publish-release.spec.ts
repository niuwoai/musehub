import { expect, test } from '@playwright/test'

import { createRepoViaUi, openRepoDelivery, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const MAIN_OID = 'b'.repeat(40)
const RUN_ID = '88888888-8888-4888-8888-888888888888'

test.describe('REL-01 publish release console', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('publish button issues create then publish when pipeline succeeded', async ({ page }) => {
    let createSeen = 0
    let publishSeen = 0

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
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({
            id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
            version: '0.0.1',
            tag_name: 'v0.0.1',
            commit_oid: MAIN_OID,
            pipeline_run_id: RUN_ID,
            state: 'draft',
            manifest_sha256: 'c'.repeat(64),
          }),
        })
        return
      }
      if (method === 'GET') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' })
        return
      }
      await route.continue()
    })

    await page.route(/\/releases\/[^/]+\/publish$/, async (route) => {
      if (route.request().method() !== 'POST') {
        await route.continue()
        return
      }
      publishSeen += 1
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
          version: '0.0.1',
          tag_name: 'v0.0.1',
          commit_oid: MAIN_OID,
          pipeline_run_id: RUN_ID,
          state: 'published',
          manifest_sha256: 'c'.repeat(64),
        }),
      })
    })

    await loginAsAdmin(page, password!)
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
    await expect(page.getByTestId('publish-pipeline-release')).toBeVisible()
    await page.getByTestId('publish-pipeline-release').click()
    await expect
      .poll(() => createSeen, { timeout: 15_000 })
      .toBe(1)
    await expect
      .poll(() => publishSeen, { timeout: 15_000 })
      .toBe(1)
  })
})
