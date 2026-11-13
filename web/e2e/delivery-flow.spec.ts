import { expect, request as playwrightRequest, test } from '@playwright/test'
import { randomUUID } from 'crypto'

import { expectMergedPullOnPullsPage, refreshRepoAndOpenDelivery } from './helpers/delivery'
import { goRepoPulls } from './helpers/navigation'
import {
  createRepoViaUi,
  openRepoBySlug,
  waitForDemoRepoNavHydrated,
} from './helpers/repo'
import { e2eBrowserApiOrigin, e2eServerOrigin } from './helpers/origin'
import { loginAsAdmin } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = () => e2eBrowserApiOrigin()
const serverApi = () => e2eServerOrigin()

test.describe('delivery chain browser flow', () => {
  test.skip(
    !password,
    'requires MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD and running musehub-server on :8080',
  )

  test('login → agent PR → second user gates → pipeline → mock deploy → trace', async ({
    page,
  }) => {
    const productOci = process.env.MUSEHUB_PRODUCT_OCI_E2E === '1'
    test.setTimeout(productOci ? 600_000 : 300_000)
    const reviewerPass = 'E2e-Reviewer-Delivery-12!'
    const ciPass = 'E2e-Ci-Delivery-12!'

    await loginAsAdmin(page, password!)

    const adminSession = (await page.context().cookies()).find((c) =>
      c.name.includes('musehub'),
    )
    expect(adminSession).toBeTruthy()
    const adminCookieHeader = `${adminSession!.name}=${adminSession!.value}`

    const invite = async (handle: string, pass: string, role: string) => {
      const r = await page.request.post(`${apiBase()}/api/v1/orgs/demo/members`, {
        headers: {
          'Content-Type': 'application/json',
          Cookie: adminCookieHeader,
        },
        data: { handle, password: pass, role },
      })
      expect(r.ok()).toBeTruthy()
    }
    await invite('e2e-reviewer', reviewerPass, 'member')
    await invite('e2e-ci', ciPass, 'admin')

    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoBySlug(page, slug)
    await page.getByRole('button', { name: /create issue|创建 issue/i }).click()
    await page.getByRole('button', { name: /accept plan|接受计划/i }).first().click()
    const runAgent = page.getByRole('button', { name: /run agent|运行/i }).first()
    await expect(runAgent).toBeVisible({ timeout: 15_000 })
    const agentStartWaitMs = 120_000
    const agentFinishWaitMs = productOci ? 300_000 : 180_000
    const agentStart = page.waitForResponse(
      (r) => r.url().includes(`/repos/${slug}/agent-runs`) && r.request().method() === 'POST',
      { timeout: agentStartWaitMs },
    )
    await runAgent.click()
    const agentStartResp = await agentStart
    expect(agentStartResp.ok()).toBeTruthy()
    const lastRun = page.getByTestId('last-agent-run')
    await expect(lastRun).toBeVisible({ timeout: 30_000 })
    await expect(lastRun).not.toContainText(/starting agent/i, { timeout: agentStartWaitMs })
    await expect(lastRun).toContainText(/succeeded/i, { timeout: agentFinishWaitMs })
    await page.getByRole('button', { name: /create pull request|创建 pull request/i }).click()
    await goRepoPulls(page)
    await expect(page.getByTestId('pull-request-list')).toContainText(/open/i, { timeout: 30_000 })
    await page.getByTestId('pull-link-1').click()
    await expect(page.getByTestId('pull-linked-issue')).toBeVisible({ timeout: 10_000 })
    await expect(page.getByTestId('pull-linked-issue')).toContainText(/#1/)
    await goRepoPulls(page)

    const loginApiCookie = async (handle: string, pass: string) => {
      const ctx = await playwrightRequest.newContext({ baseURL: serverApi() })
      const r = await ctx.post('/api/v1/auth/login', { data: { handle, password: pass } })
      expect(r.ok()).toBeTruthy()
      const setCookie = r.headers()['set-cookie'] ?? ''
      await ctx.dispose()
      return setCookie.split(';')[0]
    }
    const revCookie = await loginApiCookie('e2e-reviewer', reviewerPass)
    const ciCookie = await loginApiCookie('e2e-ci', ciPass)

    expect(slug).not.toEqual('')
    const collab = await page.request.post(
      `${apiBase()}/api/v1/orgs/demo/repos/${slug}/collaborators`,
      {
        headers: {
          'Content-Type': 'application/json',
          Cookie: adminCookieHeader,
        },
        data: { handle: 'e2e-reviewer', role: 'maintainer' },
      },
    )
    expect(collab.ok()).toBeTruthy()
    const prs = await page.request
      .get(`${apiBase()}/api/v1/orgs/demo/repos/${slug}/pull-requests`, {
        headers: { Cookie: adminCookieHeader },
      })
      .then((r) => r.json() as Promise<{ number: number }[]>)
    const prNumber = prs[0]!.number

    const review = await page.request.post(
      `${apiBase()}/api/v1/orgs/demo/repos/${slug}/pull-requests/${prNumber}/reviews`,
      {
        headers: { 'Content-Type': 'application/json', Cookie: revCookie },
        data: { decision: 'approve' },
      },
    )
    expect(review.ok()).toBeTruthy()

    const check = await page.request.patch(
      `${apiBase()}/api/v1/orgs/demo/repos/${slug}/pull-requests/${prNumber}/checks/product-ci`,
      {
        headers: { 'Content-Type': 'application/json', Cookie: ciCookie },
        data: { state: 'success', attempt_id: randomUUID() },
      },
    )
    expect(check.ok()).toBeTruthy()

    const merge = await page.request.post(
      `${apiBase()}/api/v1/orgs/demo/repos/${slug}/pull-requests/${prNumber}/merge`,
      { headers: { Cookie: revCookie } },
    )
    expect(merge.ok()).toBeTruthy()
    const mergedView = (await merge.json()) as { state: string; head_oid?: string }
    expect(mergedView.state).toBe('merged')
    expect(mergedView.head_oid).toBeTruthy()
    await expect
      .poll(
        async () => {
          const list = await page.request.get(
            `${apiBase()}/api/v1/orgs/demo/repos/${slug}/pull-requests`,
            { headers: { Cookie: adminCookieHeader } },
          )
          const body = (await list.json()) as { number: number; state: string }[]
          return body.find((p) => p.number === prNumber)?.state ?? ''
        },
        { timeout: 15_000 },
      )
      .toBe('merged')

    await expectMergedPullOnPullsPage(page)
    await refreshRepoAndOpenDelivery(page)
    const deliveryPanel = page.getByTestId('delivery-panel')
    await expect(deliveryPanel).toContainText(/main @/, { timeout: 15_000 })
    await expect(page.getByTestId('delivery-merged-issue-link')).toContainText('#1', {
      timeout: 15_000,
    })
    await page.getByRole('button', { name: /run pipeline|流水线/i }).click()
    const pipelineTimeout = productOci ? 300_000 : 120_000
    await expect(page.getByTestId('pipeline-state')).toContainText(/succeeded/i, {
      timeout: pipelineTimeout,
    })
    const releaseDeploy = page.getByRole('button', { name: /release.*mock|mock 部署/i })
    await expect(releaseDeploy).toBeEnabled({ timeout: 15_000 })
    await releaseDeploy.click()
    await expect(page.getByTestId('delivery-trace')).toBeVisible({
      timeout: productOci ? 240_000 : 90_000,
    })
    await expect(page.getByTestId('delivery-trace-issue-link')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('delivery-trace-issue-link')).toContainText(/#1/)
    await expect(page.getByTestId('delivery-trace-pr-link')).toContainText(/#1/)
    if (productOci) {
      await expect(page.getByTestId('delivery-trace')).toContainText(/compose-runner/, {
        timeout: 120_000,
      })
      await expect(page.getByTestId('delivery-trace')).toContainText(/applied_oci_digest/)
      await expect(page.getByTestId('delivery-trace')).toContainText(/verified_source_oid/)
      await expect(page.getByTestId('delivery-trace')).not.toContainText(/"mock": true/)
    } else {
      await expect(page.getByTestId('delivery-trace')).toContainText(/compose-mock/, {
        timeout: 60_000,
      })
      await expect(page.getByTestId('delivery-trace')).toContainText(/"mock": true/)
    }
  })
})
