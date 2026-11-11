import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'
import {
  createRepoViaUi,
  expectRepoSlugInSubnav,
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
  expect(planResp.ok()).toBeTruthy()
  await expect(runReferenceAgentButton(page).first()).toBeVisible({
    timeout: 15_000,
  })
  return slug
}

test.describe('agent run start repo guard', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('stale delayed agent-start 200 never renders after repo switch', async ({ page }) => {
    test.setTimeout(360_000)

    const paused = await installPausedPostAgentRunRoute(page)

    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /sign in|login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)

    const staleSlug = await prepareRepoWithAcceptedPlan(page)
    const { slug: otherSlug, repoEntry: otherEntry } = await createRepoViaUi(page)

    await openRepoBySlug(page, staleSlug)
    await expect(runReferenceAgentButton(page).first()).toBeVisible({
      timeout: 15_000,
    })

    const responseDone = page.waitForResponse(
      (r) =>
        r.url().includes(`/api/v1/orgs/demo/repos/${staleSlug}/agent-runs`) &&
        r.request().method() === 'POST',
    )

    await runReferenceAgentButton(page).first().click()
    await paused.postSeen

    await openRepoBySlug(page, otherSlug)
    await expectRepoSlugInSubnav(page, otherSlug)
    await expect(page.getByTestId('last-agent-run')).toHaveCount(0)

    paused.release()
    const staleResponse = await responseDone
    expect(staleResponse.ok()).toBeTruthy()
    const json = (await staleResponse.json()) as { id: string }
    expect(json.id).toBe(STALE_AGENT_FIXTURE_ID)

    await expect(page.getByTestId('last-agent-run')).toHaveCount(0)
    const bodyText = await page.locator('body').innerText()
    expect(bodyText).not.toContain(STALE_AGENT_FIXTURE_ID)
    expect(bodyText).not.toContain(STALE_AGENT_FIXTURE_COMMIT)

    await paused.unroute()

    await openRepoBySlug(page, staleSlug)
    await expect(runReferenceAgentButton(page).first()).toBeVisible({
      timeout: 15_000,
    })

    const recoveryPost = page.waitForResponse(
      (r) =>
        r.url().includes(`/api/v1/orgs/demo/repos/${staleSlug}/agent-runs`) &&
        r.request().method() === 'POST',
      { timeout: 180_000 },
    )
    await runReferenceAgentButton(page).first().click()
    const recoveryResponse = await recoveryPost
    expect(recoveryResponse.ok()).toBeTruthy()
    const recoveryJson = (await recoveryResponse.json()) as { id: string }
    expect(recoveryJson.id).not.toBe(STALE_AGENT_FIXTURE_ID)
    await expect(page.getByTestId('last-agent-run')).toHaveAttribute(
      'data-agent-run-id',
      recoveryJson.id,
      { timeout: 180_000 },
    )
  })
})
