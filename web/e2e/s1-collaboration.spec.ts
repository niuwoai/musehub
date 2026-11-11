import { execSync } from 'node:child_process'
import fs from 'node:fs'
import { expect, test } from '@playwright/test'
import { expectLoggedInAs } from './helpers/session'
import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'
import { apiJson, apiRaw } from './helpers/s1-api'
import { gitPushBranch } from './helpers/s1-git-seed'
import { mintGitPat, revokePat } from './helpers/s1-pat'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

async function loginAsAdmin(page: import('@playwright/test').Page) {
  await page.goto('/login', { waitUntil: 'domcontentloaded' })
  await page.getByTestId('login-handle-input').fill('admin')
  await page.getByLabel(/password|密码/i).fill(password!)
  await page.getByRole('button', { name: /login|登录/i }).click()
  await expectLoggedInAs(page, 'admin')
}

test.describe('S1 collaboration console', () => {
  test.skip(!password, 'requires bootstrap password')

  test('issues: create visible, filter notice, cursor pages', async ({ page }) => {
    await loginAsAdmin(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('create-issue').click()
    await expect(page.getByTestId('issue-link-1')).toBeVisible({ timeout: 15_000 })
    await page.getByTestId('create-issue').click()
    await expect(page.getByTestId('issue-link-2')).toBeVisible({ timeout: 15_000 })
    await page.getByTestId('create-issue').click()
    await expect(page.getByTestId('issue-link-3')).toBeVisible({ timeout: 15_000 })
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await expect(page.getByTestId('issue-row-3')).toBeVisible({ timeout: 15_000 })
    await expect(page.getByTestId('issue-row-2')).toBeVisible()
    await expect(page.getByTestId('issue-row-1')).toHaveCount(0)
    await expect(page.getByTestId('issue-load-more')).toBeVisible({ timeout: 15_000 })
    await page.getByTestId('issue-load-more').click()
    await expect(page.getByTestId('issue-row-1')).toBeVisible({ timeout: 15_000 })
    const rowCount = await page.locator('[data-testid^="issue-row-"]').count()
    expect(rowCount).toBe(3)
    await page.getByTestId('issue-link-2').click()
    await expect(page.getByTestId('issue-detail-title')).toBeVisible()
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await apiJson(page, 'PATCH', `/api/v1/orgs/demo/repos/${slug}/issues/1`, { labels: ['bug'] })
    await page.getByTestId('issue-filter-label').fill('bug')
    await page.getByTestId('issue-apply-filters').click()
    await expect(page.getByTestId('issue-row-1')).toBeVisible()
    await expect(page.getByTestId('issue-row-2')).toHaveCount(0)
    await page.getByTestId('issue-filter-label').fill('nolabel')
    await page.getByTestId('issue-apply-filters').click()
    await page.getByTestId('create-issue').click()
    await expect(page.getByTestId('issue-list-notice')).toBeVisible({ timeout: 10_000 })
  })

  test('issue detail: discussion i18n controls', async ({ page }) => {
    await loginAsAdmin(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await page.getByTestId('create-issue').click()
    await page.getByTestId('issue-link-1').click()
    await expect(page.getByTestId('issue-save-meta')).toBeVisible()
    await page.getByTestId('issue-comment-input').fill('discussion note')
    await page.getByTestId('issue-comment-submit').click()
    await expect(page.getByTestId('issue-comment')).toContainText('discussion note')
  })

  test('code: PAT git seed, oid, tree, blob', async ({ page }) => {
    await loginAsAdmin(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    const repos = await apiJson<{ id: string; slug: string; clone_url: string }[]>(
      page,
      'GET',
      '/api/v1/orgs/demo/repos',
    )
    const repo = repos.find((r) => r.slug === slug)
    expect(repo?.clone_url).toBeTruthy()
    expect(repo!.clone_url).not.toMatch(/mhp_/)
    const pat = await mintGitPat(page, `s1-git-${Date.now()}`, repo!.id)
    try {
      gitPushBranch(repo!.clone_url, pat.token, 'feature/e2e', {
        'README.md': '# e2e\nline2\n',
      })
      await page.goto(`/org/demo/repos/${slug}/code`)
      await expect(page.getByTestId('code-commit-oid')).toHaveText(/[0-9a-f]{40}/, {
        timeout: 20_000,
      })
      await page.getByTestId('code-git-ref').fill('feature/e2e')
      await page.getByTestId('code-git-ref').blur()
      await expect(page.getByTestId('code-commit-oid')).toHaveText(/[0-9a-f]{40}/, {
        timeout: 20_000,
      })
      await expect(page.getByTestId('code-entry-blob').first()).toBeVisible({ timeout: 20_000 })
      await page.getByTestId('code-entry-blob').first().click()
      await expect(page.getByTestId('code-file-body')).toContainText('e2e')
    } finally {
      await revokePat(page, pat.id)
    }
  })

  test('pull request: deep link, line comment, sync outdated', async ({ page }) => {
    await loginAsAdmin(page)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    const repos = await apiJson<{ id: string; slug: string; clone_url: string }[]>(
      page,
      'GET',
      '/api/v1/orgs/demo/repos',
    )
    const repo = repos.find((r) => r.slug === slug)!
    const pat = await mintGitPat(page, `s1-pr-${Date.now()}`, repo.id)
    try {
      gitPushBranch(repo.clone_url, pat.token, 'feature/pr-e2e', {
        'pr.txt': 'hello\nworld\n',
      })
      const pr = await apiJson<{ number: number; row_version: number }>(
        page,
        'POST',
        `/api/v1/orgs/demo/repos/${slug}/pull-requests`,
        {
          title: 'e2e pr',
          source_branch: 'feature/pr-e2e',
          target_branch: 'main',
        },
      )
      await page.goto(`/org/demo/repos/${slug}/pulls/${pr.number}`)
      await expect(page.getByTestId('pull-detail-title')).toBeVisible()
      await page.getByTestId('pull-diff-files').getByRole('button').first().click()
      await page.getByTestId('pull-diff-line-pick').first().click()
      await page.getByTestId('pull-comment-input').fill('anchored')
      await page.getByTestId('pull-comment-submit').click()
      await expect(page.getByTestId('pull-comment')).toContainText('anchored')
      gitPushBranch(repo.clone_url, pat.token, 'feature/pr-e2e', {
        'pr.txt': 'hello\nworld\nmore\n',
      })
      const detail = await apiJson<{ row_version: number }>(
        page,
        'GET',
        `/api/v1/orgs/demo/repos/${slug}/pull-requests/${pr.number}`,
      )
      const stale = await apiRaw(
        page,
        'POST',
        `/api/v1/orgs/demo/repos/${slug}/pull-requests/${pr.number}/comments`,
        {
          body: 'stale',
          anchor: {
            path: 'pr.txt',
            line: 1,
            side: 'RIGHT',
            pr_revision_id: detail.row_version - 1,
          },
        },
      )
      expect(stale.status()).toBe(400)
      await page.getByTestId('pull-sync').click()
      await expect(page.getByTestId('pull-comment').first()).toHaveAttribute(
        'data-outdated',
        'true',
      )
    } finally {
      await revokePat(page, pat.id)
    }
  })

  test('ssh keys: add and revoke', async ({ page }) => {
    await loginAsAdmin(page)
    await page.goto('/account/security')
    await expect(page.getByTestId('ssh-keys-section')).toBeVisible()
    const tmp = fs.mkdtempSync('/tmp/mh-e2e-ssh-')
    execSync(`ssh-keygen -t ed25519 -f "${tmp}/id" -N "" -q`)
    const keyMaterial = fs.readFileSync(`${tmp}/id.pub`, 'utf8').trim()
    await page.getByTestId('ssh-key-public').fill(keyMaterial)
    await page.getByTestId('ssh-key-add').click()
    await expect(page.getByTestId('ssh-key-row')).toBeVisible({ timeout: 15_000 })
    await page.getByTestId('ssh-key-row').getByRole('button').click()
    await expect(page.getByTestId('ssh-key-row')).toHaveCount(0)
  })
})
