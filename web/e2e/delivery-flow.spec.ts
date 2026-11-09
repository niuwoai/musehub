import { expect, request as playwrightRequest, test } from '@playwright/test'
import { randomUUID } from 'crypto'

import type { Page } from '@playwright/test'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

/** Repo picker in the sidebar — not issue/PR action buttons nested under the detail panel. */
function repoNavButtons(page: Page) {
  return page.locator('section > ul').first().getByRole('button')
}

test.describe('delivery chain browser flow', () => {
  test.skip(
    !password,
    'requires MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD and running musehub-server on :8080',
  )

  test('login → agent PR → second user gates → pipeline → mock deploy → trace', async ({
    page,
  }) => {
    test.setTimeout(300_000)
    const reviewerPass = 'E2e-Reviewer-Delivery-12!'
    const ciPass = 'E2e-Ci-Delivery-12!'

    await page.goto('/')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expect(page.getByText(/admin/i)).toBeVisible({ timeout: 15_000 })

    const adminSession = (await page.context().cookies()).find((c) =>
      c.name.includes('musehub'),
    )
    expect(adminSession).toBeTruthy()
    const adminCookieHeader = `${adminSession!.name}=${adminSession!.value}`

    const invite = async (handle: string, pass: string, role: string) => {
      const r = await page.request.post(`${apiBase}/api/v1/orgs/demo/members`, {
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

    const repoCountBefore = await repoNavButtons(page).count()
    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await expect(repoNavButtons(page)).toHaveCount(repoCountBefore + 1, { timeout: 15_000 })
    const repoEntry = repoNavButtons(page).last()
    await expect(repoEntry).toBeVisible({ timeout: 10_000 })
    const slug = (await repoEntry.textContent())?.trim() ?? ''
    await repoEntry.click()
    await page.getByRole('button', { name: /create issue|创建 issue/i }).click()
    await page.getByRole('button', { name: /accept plan|接受计划/i }).first().click()
    const runAgent = page.getByRole('button', { name: /run agent|运行/i }).first()
    await expect(runAgent).toBeVisible({ timeout: 15_000 })
    await runAgent.click()
    await expect(page.getByTestId('last-agent-run')).toContainText(/succeeded/i, {
      timeout: 60_000,
    })
    await page.getByRole('button', { name: /create pull request|创建 pull request/i }).click()
    await expect(page.getByTestId('pull-request-list')).toBeVisible()

    const loginApiCookie = async (handle: string, pass: string) => {
      const ctx = await playwrightRequest.newContext({ baseURL: apiBase })
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
      `${apiBase}/api/v1/orgs/demo/repos/${slug}/collaborators`,
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
      .get(`${apiBase}/api/v1/orgs/demo/repos/${slug}/pull-requests`, {
        headers: { Cookie: adminCookieHeader },
      })
      .then((r) => r.json() as Promise<{ number: number }[]>)
    const prNumber = prs[0]!.number

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
    const mergedView = (await merge.json()) as { state: string; head_oid?: string }
    expect(mergedView.state).toBe('merged')
    expect(mergedView.head_oid).toBeTruthy()
    await expect
      .poll(
        async () => {
          const list = await page.request.get(
            `${apiBase}/api/v1/orgs/demo/repos/${slug}/pull-requests`,
            { headers: { Cookie: adminCookieHeader } },
          )
          const body = (await list.json()) as { number: number; state: string }[]
          return body.find((p) => p.number === prNumber)?.state ?? ''
        },
        { timeout: 15_000 },
      )
      .toBe('merged')

    await page.getByTestId('refresh-repo').click()
    const deliveryPanel = page.getByTestId('delivery-panel')
    await expect(page.getByTestId('pull-request-list')).toContainText(/merged/i, {
      timeout: 30_000,
    })
    await expect(deliveryPanel).toBeVisible()
    await expect(deliveryPanel).toContainText(/main @/, { timeout: 15_000 })
    await page.getByRole('button', { name: /run pipeline|流水线/i }).click()
    await expect(page.getByTestId('pipeline-state')).toContainText(/succeeded/i, {
      timeout: 120_000,
    })
    await page.getByRole('button', { name: /release.*mock|mock 部署/i }).click()
    await expect(page.getByTestId('delivery-trace')).toContainText(/compose-mock/, {
      timeout: 60_000,
    })
    await expect(page.getByTestId('delivery-trace')).toContainText(/"mock": true/)
  })
})
