import { expect, test } from '@playwright/test'

import { expectLoggedInAs } from './helpers/session'
import { createIssueViaUi } from './helpers/issues'
import {
  createRepoViaUi,
  expectResponseOk,
  openRepoBySlug,
  waitForDemoRepoNavHydrated,
} from './helpers/repo'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('product slice browser flow', () => {
  test.skip(
    !password,
    'requires MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD and running musehub-server on :8080',
  )

  test('login, repo, issue, plan, agent poll', async ({ page }) => {
    test.setTimeout(180_000)

    await page.goto('/login')
    await page.getByLabel(/password|密码/i).fill(password!)
    await page.getByRole('button', { name: /login|登录/i }).click()
    await expectLoggedInAs(page, 'admin')
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await openRepoBySlug(page, slug)

    await createIssueViaUi(page, 'product slice acceptance', { slug })
    await expect(page.getByTestId('issue-row-1')).toBeVisible({ timeout: 15_000 })

    const planCreate = page.waitForResponse(
      (r) =>
        r.url().includes(`/repos/${slug}/issues/1/plans`) &&
        r.request().method() === 'POST' &&
        r.ok(),
    )
    const planAccept = page.waitForResponse(
      (r) =>
        r.url().includes(`/repos/${slug}/issues/1/plans`) &&
        r.request().method() === 'PATCH' &&
        r.status() === 204,
    )
    await page
      .getByTestId('issue-row-1')
      .getByRole('button', { name: /accept plan|接受计划/i })
      .click()
    await planCreate
    await planAccept

    const runAgent = page
      .getByTestId('issue-row-1')
      .getByRole('button', { name: /run agent|运行|reference/i })
    await expect(runAgent).toBeVisible({ timeout: 15_000 })
    const agentStart = page.waitForResponse(
      (r) => r.url().includes(`/repos/${slug}/agent-runs`) && r.request().method() === 'POST',
      { timeout: 60_000 },
    )
    await runAgent.click()
    const agentStartResp = await agentStart
    await expectResponseOk(agentStartResp, 'start agent run')
    const started = (await agentStartResp.json()) as { id?: string; state?: string }
    expect(started.id?.length).toBeGreaterThan(0)

    const lastRun = page.getByTestId('last-agent-run')
    await expect(lastRun).toHaveAttribute('data-agent-run-id', started.id!, {
      timeout: 30_000,
    })
    await expect(lastRun).not.toHaveText(/^\s*$/, { timeout: 30_000 })

    await expect
      .poll(
        async () => {
          const runId = await lastRun.getAttribute('data-agent-run-id')
          if (runId !== started.id) {
            return ''
          }
          return await lastRun.innerText()
        },
        { timeout: 120_000, intervals: [500, 1000, 2000] },
      )
      .toMatch(/succeeded/i)

    const me = await page.request.get('/api/v1/me')
    expect(me.ok()).toBeTruthy()
  })
})
