import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'
import {
  createRepoViaUi,
  expectRepoSlugInSubnav,
  expectResponseOk,
  openRepoBySlug,
  waitForDemoRepoNavHydrated,
} from './helpers/repo'
import {
  STALE_AGENT_FIXTURE_BODY,
  STALE_AGENT_FIXTURE_COMMIT,
  STALE_AGENT_FIXTURE_ID,
} from './fixtures/stale-agent-run'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

async function installPausedPostAgentRunRoute(page: import('@playwright/test').Page) {
  let releaseGate: (() => void) | undefined
  const gate = new Promise<void>((resolve) => {
    releaseGate = resolve
  })
  let postSeenResolve: () => void = () => {}
  const postSeen = new Promise<void>((resolve) => {
    postSeenResolve = resolve
  })
  let pauseFirstPost = true

  const pattern = '**/api/v1/orgs/demo/repos/*/agent-runs'
  await page.route(pattern, async (route) => {
    if (route.request().method() === 'POST' && pauseFirstPost) {
      postSeenResolve()
      await gate
      pauseFirstPost = false
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STALE_AGENT_FIXTURE_BODY),
      })
      return
    }
    await route.continue()
  })

  return {
    postSeen,
    release: () => releaseGate!(),
    unroute: async () => {
      pauseFirstPost = false
      releaseGate?.()
      await page.unroute(pattern)
    },
  }
}

const runReferenceAgentButton = (page: import('@playwright/test').Page) =>
  page.getByRole('button', {
    name: /run reference agent|运行参考 agent/i,
  })

async function prepareRepoWithAcceptedPlan(page: import('@playwright/test').Page) {
  await waitForDemoRepoNavHydrated(page)
  const { slug } = await createRepoViaUi(page)
  await openRepoBySlug(page, slug)
  await expectRepoSlugInSubnav(page, slug)
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

test.describe('agent run start session guard', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('stale delayed agent-start 200 never renders; next session can start a run', async ({
    page,
  }) => {
    test.setTimeout(360_000)

    const paused = await installPausedPostAgentRunRoute(page)

    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    const repoSlug = await prepareRepoWithAcceptedPlan(page)

    const responseDone = page.waitForResponse(
      (r) =>
        r.url().includes('/api/v1/orgs/demo/repos/') &&
        r.url().includes('/agent-runs') &&
        r.request().method() === 'POST',
    )

    await runReferenceAgentButton(page).first().click()
    await paused.postSeen

    await page.getByTestId('logout').click()
    await expect(page.getByTestId('session-handle')).toHaveCount(0)

    paused.release()
    const staleResponse = await responseDone
    expect(staleResponse.ok()).toBeTruthy()
    const json = (await staleResponse.json()) as { id: string }
    expect(json.id).toBe(STALE_AGENT_FIXTURE_ID)

    await expect(page.getByTestId('last-agent-run')).toHaveCount(0)
    const loggedOutBody = await page.locator('body').innerText()
    expect(loggedOutBody).not.toContain(STALE_AGENT_FIXTURE_ID)
    expect(loggedOutBody).not.toContain(STALE_AGENT_FIXTURE_COMMIT)

    await paused.unroute()

    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    await openRepoBySlug(page, repoSlug)
    await expect(runReferenceAgentButton(page).first()).toBeVisible({
      timeout: 15_000,
    })
    const runAgentButton = runReferenceAgentButton(page).first()
    await expect(runAgentButton).toBeEnabled({ timeout: 15_000 })

    const realStart = page.waitForResponse(
      (r) =>
        r.url().includes(`/repos/${repoSlug}/agent-runs`) &&
        r.request().method() === 'POST',
      { timeout: 180_000 },
    )
    await runAgentButton.click()
    const realResponse = await realStart
    expect(realResponse.ok()).toBeTruthy()
    const realJson = (await realResponse.json()) as { id: string }
    expect(realJson.id).not.toBe(STALE_AGENT_FIXTURE_ID)
    await expect(page.getByTestId('last-agent-run')).toBeVisible({ timeout: 180_000 })
    const lastRunText = await page.getByTestId('last-agent-run').innerText()
    expect(lastRunText).not.toContain(STALE_AGENT_FIXTURE_ID)
    expect(lastRunText).not.toContain(STALE_AGENT_FIXTURE_COMMIT)
  })
})
