import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'
import {
  STALE_AGENT_FIXTURE_BODY,
  STALE_AGENT_FIXTURE_COMMIT,
  STALE_AGENT_FIXTURE_ID,
} from './fixtures/stale-agent-run'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

function repoNavButtons(page: import('@playwright/test').Page) {
  return page.locator('section > ul').first().getByRole('button')
}

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
  const repoCountBefore = await repoNavButtons(page).count()
  await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
  await expect(repoNavButtons(page)).toHaveCount(repoCountBefore + 1, { timeout: 15_000 })
  const repoEntry = repoNavButtons(page).last()
  const slug = (await repoEntry.textContent())?.trim() ?? ''
  await repoEntry.click()
  await expect(page.getByRole('heading', { level: 2, name: slug })).toBeVisible({
    timeout: 15_000,
  })
  await page.getByRole('button', { name: /create issue|创建 issue/i }).click()
  const planAccepted = page.waitForResponse(
    (r) =>
      r.url().includes(`/repos/${slug}/issues/`) &&
      r.url().includes('/plans') &&
      r.request().method() === 'PATCH' &&
      r.ok(),
  )
  await page.getByRole('button', { name: /accept plan|接受计划/i }).first().click()
  await planAccepted
  await expect(runReferenceAgentButton(page).first()).toBeVisible({
    timeout: 15_000,
  })
  return slug
}

async function openRepoWithSlug(page: import('@playwright/test').Page, slug: string) {
  const issuesLoaded = page.waitForResponse(
    (r) => r.url().includes(`/repos/${slug}/issues`) && r.request().method() === 'GET' && r.ok(),
  )
  await repoNavButtons(page).filter({ hasText: slug }).click()
  await issuesLoaded
  await expect(page.getByRole('heading', { level: 2, name: slug })).toBeVisible({
    timeout: 15_000,
  })
  await expect(runReferenceAgentButton(page).first()).toBeVisible({
    timeout: 15_000,
  })
}

test.describe('agent run start session guard', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('stale delayed agent-start 200 never renders; next session can start a run', async ({
    page,
  }) => {
    test.setTimeout(360_000)

    const paused = await installPausedPostAgentRunRoute(page)

    await page.goto('/')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
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
    await openRepoWithSlug(page, repoSlug)
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
