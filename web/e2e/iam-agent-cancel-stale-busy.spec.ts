import { expect, test } from '@playwright/test'
import { randomUUID } from 'crypto'

import { expectLoggedInAs } from './helpers/session'
import {
  createRepoViaUi,
  expectResponseOk,
  openRepoBySlug,
  waitForDemoRepoNavHydrated,
} from './helpers/repo'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

const STUCK_RUN_ID = randomUUID()

const runReferenceAgentButton = (page: import('@playwright/test').Page) =>
  page.getByRole('button', {
    name: /run reference agent|运行参考 agent/i,
  })

const cancelAgentButton = (page: import('@playwright/test').Page) =>
  page.getByRole('button', {
    name: /cancel agent run|取消 agent/i,
  })

async function prepareRepoWithAcceptedPlan(page: import('@playwright/test').Page) {
  await waitForDemoRepoNavHydrated(page)
  const { slug } = await createRepoViaUi(page)
  await openRepoBySlug(page, slug)
  await page.getByRole('button', { name: /create issue|创建 issue/i }).click()
  const planAccepted = page.waitForResponse(
    (r) =>
      r.url().includes(`/repos/${slug}/issues/`) &&
      r.url().includes('/plans') &&
      r.request().method() === 'PATCH',
  )
  await page.getByRole('button', { name: /accept plan|接受计划/i }).first().click()
  const planResp = await planAccepted
  await expectResponseOk(planResp, 'accept plan')
  await expect(runReferenceAgentButton(page).first()).toBeVisible({
    timeout: 15_000,
  })
  return slug
}

test.describe('agent cancel must not clear a newer start busy', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('slow cancel poll while second start keeps new run identity', async ({ page }) => {
    test.setTimeout(360_000)

    let cancelPollGate: (() => void) | undefined
    const cancelPollHold = new Promise<void>((resolve) => {
      cancelPollGate = resolve
    })
    let cancelApiSeen = false
    let agentPostCount = 0
    const runUrl = `**/api/v1/agent-runs/${STUCK_RUN_ID}`

    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /sign in|login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')

    const slug = await prepareRepoWithAcceptedPlan(page)
    const postsUrl = `**/api/v1/orgs/demo/repos/${slug}/agent-runs`

    await page.route(postsUrl, async (route) => {
      if (route.request().method() !== 'POST') {
        await route.continue()
        return
      }
      agentPostCount += 1
      if (agentPostCount === 1) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            id: STUCK_RUN_ID,
            adapter_kind: 'reference',
            state: 'running',
            result_commit_oid: null,
            usage_tokens: 0,
            budget_unknown: true,
          }),
        })
        return
      }
      await route.continue()
    })

    await page.route(runUrl, async (route) => {
      if (route.request().method() !== 'GET') {
        await route.continue()
        return
      }
      if (cancelApiSeen) {
        await cancelPollHold
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            id: STUCK_RUN_ID,
            adapter_kind: 'reference',
            state: 'canceled',
            result_commit_oid: null,
            usage_tokens: 0,
            budget_unknown: true,
          }),
        })
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: STUCK_RUN_ID,
          adapter_kind: 'reference',
          state: 'running',
          result_commit_oid: null,
          usage_tokens: 0,
          budget_unknown: true,
        }),
      })
    })

    await page.route(`${runUrl}/cancel`, async (route) => {
      cancelApiSeen = true
      await route.fulfill({ status: 204, body: '' })
    })

    await runReferenceAgentButton(page).first().click()
    await expect(page.getByTestId('last-agent-run')).toHaveAttribute(
      'data-agent-run-id',
      STUCK_RUN_ID,
      { timeout: 15_000 },
    )
    await expect(cancelAgentButton(page)).toBeVisible()

    const secondPost = page.waitForResponse(
      (r) =>
        r.url().includes(`/repos/${slug}/agent-runs`) &&
        r.request().method() === 'POST',
      { timeout: 180_000 },
    )

    await cancelAgentButton(page).click()
    await expect.poll(() => cancelApiSeen).toBe(true)

    await runReferenceAgentButton(page).first().click()
    const secondResponse = await secondPost
    await expectResponseOk(secondResponse, 'second agent start POST')
    const secondJson = (await secondResponse.json()) as { id: string }
    expect(secondJson.id).not.toBe(STUCK_RUN_ID)

    await expect(page.getByTestId('last-agent-run')).toHaveAttribute(
      'data-agent-run-id',
      secondJson.id,
      { timeout: 30_000 },
    )

    cancelPollGate?.()
    await expect(page.getByTestId('last-agent-run')).toHaveAttribute(
      'data-agent-run-id',
      secondJson.id,
    )
    await expect(page.getByTestId('last-agent-run')).not.toHaveAttribute(
      'data-agent-run-id',
      STUCK_RUN_ID,
    )

    await page.unroute(postsUrl)
    await page.unroute(runUrl)
    await page.unroute(`${runUrl}/cancel`)
  })
})
