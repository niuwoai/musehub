import { expect, test } from '@playwright/test'
import { randomUUID } from 'crypto'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

test.describe('delivery chain browser flow', () => {
  test.skip(
    !password,
    'requires MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD and running musehub-server on :8080',
  )

  test('login → agent PR → second user gates → pipeline → mock deploy → trace', async ({
    page,
  }) => {
    const reviewerPass = 'E2e-Reviewer-Delivery-12!'
    const ciPass = 'E2e-Ci-Delivery-12!'

    await page.goto('/')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expect(page.getByText(/admin/i)).toBeVisible({ timeout: 15_000 })

    const cookies = await page.context().cookies()
    const sessionCookie = cookies.find((c) => c.name.includes('musehub'))
    expect(sessionCookie).toBeTruthy()

    const invite = async (handle: string, pass: string, role: string) => {
      const r = await page.request.post(`${apiBase}/api/v1/orgs/demo/members`, {
        headers: {
          'Content-Type': 'application/json',
          Cookie: `${sessionCookie!.name}=${sessionCookie!.value}`,
        },
        data: { handle, password: pass, role },
      })
      expect(r.ok()).toBeTruthy()
    }
    await invite('e2e-reviewer', reviewerPass, 'member')
    await invite('e2e-ci', ciPass, 'admin')

    await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
    await page.locator('ul li button').first().click()
    await page.getByRole('button', { name: /create issue|创建 issue/i }).click()
    await page.getByRole('button', { name: /accept plan|接受计划/i }).click()
    await page.getByRole('button', { name: /run agent|运行/i }).first().click()
    await expect(page.getByTestId('last-agent-run')).toContainText(/succeeded/i, {
      timeout: 60_000,
    })
    await page.getByRole('button', { name: /create pull request|创建 pull request/i }).click()
    await expect(page.getByTestId('pull-request-list')).toBeVisible()

    const loginApi = async (handle: string, pass: string) => {
      const r = await page.request.post(`${apiBase}/api/v1/auth/login`, {
        data: { handle, password: pass },
      })
      expect(r.ok()).toBeTruthy()
      const setCookie = r.headers()['set-cookie'] ?? ''
      return setCookie.split(';')[0]
    }
    const revCookie = await loginApi('e2e-reviewer', reviewerPass)
    const ciCookie = await loginApi('e2e-ci', ciPass)

    const repoSlug = (
      await page.request.get(`${apiBase}/api/v1/orgs/demo/repos`, {
        headers: { Cookie: `${sessionCookie!.name}=${sessionCookie!.value}` },
      })
    )
      .json()
      .then((repos: { slug: string }[]) => repos[0]!.slug)

    const slug = await repoSlug
    const collab = await page.request.post(
      `${apiBase}/api/v1/orgs/demo/repos/${slug}/collaborators`,
      {
        headers: {
          'Content-Type': 'application/json',
          Cookie: `${sessionCookie!.name}=${sessionCookie!.value}`,
        },
        data: { handle: 'e2e-reviewer', role: 'maintainer' },
      },
    )
    expect(collab.ok()).toBeTruthy()
    const prs = await page.request
      .get(`${apiBase}/api/v1/orgs/demo/repos/${slug}/pull-requests`, {
        headers: { Cookie: `${sessionCookie!.name}=${sessionCookie!.value}` },
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

    await page.reload()
    await page.locator('ul li button').first().click()
    await expect(page.getByTestId('delivery-panel')).toBeVisible()
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
