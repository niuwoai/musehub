import { expect, request as playwrightRequest, test } from '@playwright/test'
import { randomUUID } from 'crypto'

import type { Page } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'
import {
  createRepoViaUi,
  repoNavButtonForSlug,
  waitForDemoRepoNavHydrated,
} from './helpers/repo'

const adminPassword = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

async function fillLoginForm(page: Page, handle: string, pass: string) {
  const loginForm = page.locator('form').filter({ has: page.getByTestId('login-handle-input') })
  await loginForm.getByLabel(/password|密码/i).fill(pass)
  await page.getByTestId('login-handle-input').fill(handle)
}

async function loginCookie(handle: string, pass: string) {
  const ctx = await playwrightRequest.newContext({ baseURL: apiBase })
  const r = await ctx.post('/api/v1/auth/login', { data: { handle, password: pass } })
  expect(r.ok()).toBeTruthy()
  const setCookie = r.headers()['set-cookie'] ?? ''
  await ctx.dispose()
  return setCookie.split(';')[0]
}

async function deliveryThroughRelease(page: Page, adminCookie: string) {
  const { slug, repoEntry } = await createRepoViaUi(page)
  await repoEntry.click()
  await page.getByRole('button', { name: /create issue|创建 issue/i }).click()
  await page.getByRole('button', { name: /accept plan|接受计划/i }).first().click()
  await page.getByRole('button', { name: /run agent|运行/i }).first().click()
  await expect(page.getByTestId('last-agent-run')).toContainText(/succeeded/i, {
    timeout: 180_000,
  })
  await page.getByRole('button', { name: /create pull request|创建 pull request/i }).click()
  await expect(page.getByTestId('pull-request-list')).toBeVisible({ timeout: 30_000 })
  const prs = await page.request.get(`${apiBase}/api/v1/orgs/demo/repos/${slug}/pull-requests`, {
    headers: { Cookie: adminCookie },
  })
  expect(prs.ok()).toBeTruthy()
  const prList = (await prs.json()) as { number: number }[]
  expect(prList.length).toBeGreaterThan(0)
  const prNumber = prList[0]!.number
  const reviewerPass = 'Dep-Reviewer-22!'
  const ciPass = 'Dep-Ci-Reporter-22!'
  const memSuffix = randomUUID().replace(/-/g, '').slice(0, 12)
  const revHandle = `dep-rev-${memSuffix}`
  const ciHandle = `dep-ci-${memSuffix}`
  const memberRev = await page.request.post(`${apiBase}/api/v1/orgs/demo/members`, {
    headers: { 'Content-Type': 'application/json', Cookie: adminCookie },
    data: { handle: revHandle, password: reviewerPass, role: 'member' },
  })
  expect(memberRev.ok()).toBeTruthy()
  const memberCi = await page.request.post(`${apiBase}/api/v1/orgs/demo/members`, {
    headers: { 'Content-Type': 'application/json', Cookie: adminCookie },
    data: { handle: ciHandle, password: ciPass, role: 'admin' },
  })
  expect(memberCi.ok()).toBeTruthy()
  const collab = await page.request.post(
    `${apiBase}/api/v1/orgs/demo/repos/${slug}/collaborators`,
    {
      headers: { 'Content-Type': 'application/json', Cookie: adminCookie },
      data: { handle: revHandle, role: 'maintainer' },
    },
  )
  expect(collab.ok()).toBeTruthy()
  const revCookie = await loginCookie(revHandle, reviewerPass)
  const ciCookie = await loginCookie(ciHandle, ciPass)
  const review = await page.request.post(
    `${apiBase}/api/v1/orgs/demo/repos/${slug}/pull-requests/${prNumber}/reviews`,
    {
      headers: { 'Content-Type': 'application/json', Cookie: revCookie },
      data: { decision: 'approve' },
    },
  )
  expect(review.ok()).toBeTruthy()
  const check = await page.request.patch(
    `${apiBase}/api/v1/orgs/demo/repos/${slug}/pull-requests/${prNumber}/checks/product-ci`,
    {
      headers: { 'Content-Type': 'application/json', Cookie: ciCookie },
      data: { state: 'success', attempt_id: randomUUID() },
    },
  )
  expect(check.ok()).toBeTruthy()
  const merge = await page.request.post(
    `${apiBase}/api/v1/orgs/demo/repos/${slug}/pull-requests/${prNumber}/merge`,
    { headers: { Cookie: revCookie } },
  )
  expect(merge.ok()).toBeTruthy()
  await page.getByTestId('refresh-repo').click()
  await expect(page.getByTestId('delivery-panel')).toContainText(/main @/, { timeout: 30_000 })
  await page.getByRole('button', { name: /run pipeline|流水线/i }).click()
  await expect(page.getByTestId('pipeline-state')).toContainText(/succeeded/i, {
    timeout: 120_000,
  })
  const releaseResp = page.waitForResponse(
    (r) =>
      r.url().includes('/releases') &&
      r.request().method() === 'POST' &&
      r.url().includes(slug),
  )
  await page.getByRole('button', { name: /release.*mock|mock 部署/i }).click()
  const rel = await releaseResp
  expect(rel.ok()).toBeTruthy()
  const relBody = (await rel.json()) as { id: string }
  return { slug, releaseId: relBody.id }
}

const STALE_PROD_PLAN_PREFIX = 'cccccccccccc'

test.describe('protected deployment approval console', () => {
  test.skip(!adminPassword, 'requires bootstrap password and API on :8080')

  test('stale delayed production deploy start cleared on logout', async ({ page }) => {
    test.setTimeout(360_000)
    let releasePost: () => void = () => {}
    const postGate = new Promise<void>((resolve) => {
      releasePost = resolve
    })
    let postSeenResolve: () => void = () => {}
    const postSeen = new Promise<void>((resolve) => {
      postSeenResolve = resolve
    })

    await page.route('**/deployments', async (route) => {
      if (route.request().method() !== 'POST') {
        await route.continue()
        return
      }
      const body = route.request().postDataJSON() as { environment_slug?: string }
      if (body.environment_slug !== 'production') {
        await route.continue()
        return
      }
      postSeenResolve()
      await postGate
      const staleHash = `${STALE_PROD_PLAN_PREFIX}${'c'.repeat(52)}`
      await route.fulfill({
        status: 202,
        contentType: 'application/json',
        body: JSON.stringify({
          id: '44444444-4444-4444-8444-444444444444',
          environment_slug: 'production',
          release_id: body.release_id ?? '22222222-2222-4222-8222-222222222222',
          state: 'awaiting_approval',
          adapter_kind: 'compose-mock',
          plan_hash: staleHash,
          approval_required: true,
        }),
      })
    })

    await page.goto('/')
    await fillLoginForm(page, 'admin', adminPassword!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    const adminCookie = `${(await page.context().cookies()).find((c) => c.name.includes('musehub'))!.name}=${(await page.context().cookies()).find((c) => c.name.includes('musehub'))!.value}`

    await deliveryThroughRelease(page, adminCookie)
    const staleProd = page.waitForResponse(
      (r) =>
        r.url().includes('/deployments') &&
        r.request().method() === 'POST' &&
        r.status() === 202,
    )
    void page.getByTestId('deploy-production').click()
    await postSeen
    await page.getByTestId('logout').click()
    releasePost()
    await staleProd

    await fillLoginForm(page, 'admin', adminPassword!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    await expect(page.getByTestId('deployment-approval')).toHaveCount(0)
    const body = await page.locator('body').innerText()
    expect(body).not.toContain(STALE_PROD_PLAN_PREFIX)

    await page.unroute('**/deployments')
  })

  test('approver acts in console; denied member and stale session guarded', async ({ page }) => {
    test.setTimeout(360_000)
    const suffix = Date.now().toString(36)
    const requesterHandle = `dep-req-${suffix}`
    const deniedHandle = `dep-deny-${suffix}`
    const requesterPass = 'Dep-Requester-22!'
    const deniedPass = 'Dep-Denied-Mbr-22!'

    await page.goto('/')
    await fillLoginForm(page, 'admin', adminPassword!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    const adminSession = (await page.context().cookies()).find((c) =>
      c.name.includes('musehub'),
    )
    expect(adminSession).toBeTruthy()
    const adminCookie = `${adminSession!.name}=${adminSession!.value}`

    for (const [handle, pass, role] of [
      [requesterHandle, requesterPass, 'member'],
      [deniedHandle, deniedPass, 'member'],
    ] as const) {
      const invite = await page.request.post(`${apiBase}/api/v1/orgs/demo/members`, {
        headers: { 'Content-Type': 'application/json', Cookie: adminCookie },
        data: { handle, password: pass, role },
      })
      expect(invite.ok()).toBeTruthy()
    }

    const { slug, releaseId } = await deliveryThroughRelease(page, adminCookie)

    const grant = await page.request.post(
      `${apiBase}/api/v1/orgs/demo/repos/${slug}/collaborators`,
      {
        headers: { 'Content-Type': 'application/json', Cookie: adminCookie },
        data: { handle: requesterHandle, role: 'developer' },
      },
    )
    expect(grant.ok()).toBeTruthy()

    const requesterCookie = await loginCookie(requesterHandle, requesterPass)
    const start = await page.request.post(
      `${apiBase}/api/v1/orgs/demo/repos/${slug}/deployments`,
      {
        headers: { 'Content-Type': 'application/json', Cookie: requesterCookie },
        data: {
          release_id: releaseId,
          environment_slug: 'production',
          attempt_id: randomUUID(),
        },
      },
    )
    expect(start.status()).toBe(202)
    const pending = (await start.json()) as {
      id: string
      state: string
      plan_hash: string
      approval_required: boolean
    }
    expect(pending.state).toBe('awaiting_approval')
    expect(pending.approval_required).toBe(true)
    expect(pending.plan_hash?.length).toBe(64)

    const deniedCookie = await loginCookie(deniedHandle, deniedPass)
    const deniedApprove = await page.request.post(
      `${apiBase}/api/v1/orgs/demo/repos/${slug}/deployments/${pending.id}/approval`,
      {
        headers: { 'Content-Type': 'application/json', Cookie: deniedCookie },
        data: {
          expected_plan_hash: pending.plan_hash,
          decision: 'approve',
          reason: 'should deny',
        },
      },
    )
    expect(deniedApprove.status()).toBe(403)

    await page.getByTestId('logout').click()
    await fillLoginForm(page, 'admin', adminPassword!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    await repoNavButtonForSlug(page, slug).click()
    await page.getByTestId('deployment-lookup-id').fill(pending.id)
    await page.getByTestId('load-deployment').click()
    await expect(page.getByTestId('deployment-approval')).toBeVisible()
    await page.getByTestId('logout').click()
    await expect(page.getByTestId('deployment-approval')).toHaveCount(0)
    const loggedOutBody = await page.locator('body').innerText()
    expect(loggedOutBody).not.toContain(pending.plan_hash.slice(0, 12))

    await fillLoginForm(page, 'admin', adminPassword!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    await repoNavButtonForSlug(page, slug).click()
    await page.getByTestId('deployment-lookup-id').fill(pending.id)
    await page.getByTestId('load-deployment').click()
    await expect(page.getByTestId('deployment-approval')).toBeVisible()
    await Promise.all([
      page.waitForResponse(
        (r) =>
          r.url().includes(`/deployments/${pending.id}/approval`) && r.request().method() === 'POST',
      ),
      page.getByTestId('approve-deployment').click(),
    ])
    await expect(page.getByTestId('deployment-approval')).toHaveCount(0, { timeout: 30_000 })
    await expect
      .poll(
        async () => {
          const dep = await page.request.get(
            `${apiBase}/api/v1/orgs/demo/repos/${slug}/deployments/${pending.id}`,
            { headers: { Cookie: adminCookie } },
          )
          const j = (await dep.json()) as { state: string }
          return j.state
        },
        { timeout: 90_000 },
      )
      .not.toBe('awaiting_approval')

    const repeat = await page.request.post(
      `${apiBase}/api/v1/orgs/demo/repos/${slug}/deployments/${pending.id}/approval`,
      {
        headers: { 'Content-Type': 'application/json' },
        data: {
          expected_plan_hash: pending.plan_hash,
          decision: 'approve',
          reason: 'repeat',
        },
      },
    )
    expect(repeat.status()).toBe(409)
  })
})
