import { expect, test } from '@playwright/test'
import { randomUUID } from 'crypto'

import { expectLoggedInAs } from './helpers/session'
import {
  createRepoViaUi,
  expectResponseOk,
  waitForDemoRepoNavHydrated,
} from './helpers/repo'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

const CONTROLLABLE_RUN_ID = randomUUID()

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
  const { slug, repoEntry } = await createRepoViaUi(page)
  await repoEntry.click()
  await expect(page.getByRole('heading', { level: 2, name: slug })).toBeVisible({
    timeout: 15_000,
  })
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

async function installFirstRunThenRealPosts(
  page: import('@playwright/test').Page,
  slug: string,
) {
  let agentPostCount = 0
  let cancelSeen = false
  const runUrl = `**/api/v1/agent-runs/${CONTROLLABLE_RUN_ID}`
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
          id: CONTROLLABLE_RUN_ID,
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
    const state = cancelSeen ? 'canceled' : 'running'
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        id: CONTROLLABLE_RUN_ID,
        adapter_kind: 'reference',
        state,
        result_commit_oid: null,
        usage_tokens: 0,
        budget_unknown: true,
      }),
    })
  })

  await page.route(`${runUrl}/cancel`, async (route) => {
    cancelSeen = true
    await route.fulfill({ status: 204, body: '' })
  })

  return {
    getPostCount: () => agentPostCount,
    unroute: async () => {
      await page.unroute(postsUrl)
      await page.unroute(runUrl)
      await page.unroute(`${runUrl}/cancel`)
    },
  }
}

test.describe('agent cancel releases busy for next start', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('mid-poll cancel then second agent-runs POST', async ({ page }) => {
    test.setTimeout(360_000)

    await page.goto('/')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /sign in|login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')

    const slug = await prepareRepoWithAcceptedPlan(page)
    const routes = await installFirstRunThenRealPosts(page, slug)

    const firstPost = page.waitForResponse(
      (r) =>
        r.url().includes(`/repos/${slug}/agent-runs`) &&
        r.request().method() === 'POST',
    )
    await runReferenceAgentButton(page).first().click()
    await firstPost
    await expect(page.getByTestId('last-agent-run')).toContainText(/running/i, {
      timeout: 15_000,
    })
    await expect(cancelAgentButton(page)).toBeVisible({ timeout: 15_000 })

    const secondPost = page.waitForResponse(
      (r) =>
        r.url().includes(`/repos/${slug}/agent-runs`) &&
        r.request().method() === 'POST',
      { timeout: 180_000 },
    )

    await cancelAgentButton(page).click()
    await expect(page.getByTestId('last-agent-run')).toContainText(/canceled/i, {
      timeout: 60_000,
    })
    await expect(page.getByTestId('last-agent-run')).toHaveAttribute(
      'data-agent-run-id',
      CONTROLLABLE_RUN_ID,
    )

    await runReferenceAgentButton(page).first().click()
    const secondResponse = await secondPost
    await expectResponseOk(secondResponse, 'second agent start POST')
    const secondJson = (await secondResponse.json()) as { id: string }
    expect(secondJson.id).not.toBe(CONTROLLABLE_RUN_ID)
    expect(routes.getPostCount()).toBeGreaterThanOrEqual(2)

    await routes.unroute()
    await expect(page.getByTestId('last-agent-run')).toHaveAttribute(
      'data-agent-run-id',
      secondJson.id,
      { timeout: 180_000 },
    )
    await expect(page.getByTestId('last-agent-run')).not.toContainText(/canceled/i)
  })
})
