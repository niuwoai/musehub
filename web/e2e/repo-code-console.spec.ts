import { expect, test } from '@playwright/test'

import { createRepoViaUi, openRepoBySlug, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin } from './helpers/session'
import { gitPushCommits } from './helpers/s1-git-seed'
import { mintGitPat, revokePat } from './helpers/s1-pat'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const README_FIXTURE = '# repo-code-console\nfixture line\n'

test.describe('repo code browser', () => {
  test.skip(!password, 'requires bootstrap password')

  test('browse seeded main tree and read text blob', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    const repos = (await (
      await page.request.get('/api/v1/orgs/demo/repos')
    ).json()) as Array<{ id: string; slug: string; clone_url: string; ssh_clone_url: string }>
    const repo = repos.find((r) => r.slug === slug)
    expect(repo?.clone_url).toBeTruthy()
    expect(repo?.ssh_clone_url).toMatch(/^ssh:\/\//)
    const pat = await mintGitPat(page, `code-ui-${Date.now()}`, repo!.id)
    try {
      const branch = 'feature/repo-code-console'
      gitPushCommits(
        repo!.clone_url,
        pat.token,
        branch,
        Array.from({ length: 21 }, (_, index) => ({
          'README.md': `${README_FIXTURE}history commit ${index + 1}\n`,
          ...(index === 20 ? { 'docs/notes with spaces.txt': 'diff path fixture\n' } : {}),
        })),
      )
      await openRepoBySlug(page, slug)
      await page.getByTestId('nav-repo-code').click()
      await expect(page.getByTestId('code-http-clone-url')).toBeVisible({ timeout: 15_000 })
      await expect(page.getByTestId('code-http-clone-url')).toContainText(repo!.clone_url)
      await expect(page.getByTestId('code-ssh-clone-url')).toBeVisible()
      await expect(page.getByTestId('code-ssh-clone-url')).toContainText(repo!.ssh_clone_url)
      await expect(page.getByTestId('code-ssh-keys-link')).toBeVisible()
      await page.getByTestId('code-git-ref').fill(branch)
      await page.getByTestId('code-git-ref').blur()
      await expect(page.getByTestId('repo-code')).toBeVisible({ timeout: 15_000 })
      await expect(page.getByTestId('code-pinned-commit')).toBeVisible()
      await expect(page.getByTestId('code-commit-oid')).toHaveText(/[0-9a-f]{40}/, {
        timeout: 20_000,
      })
      const history = page.getByTestId('code-history-commit')
      await expect(history).toHaveCount(20)
      const firstPageOids = await history.locator('code').allTextContents()
      await page.getByTestId('code-history-load-more').click()
      await expect(history).toHaveCount(22)
      await expect(page.getByTestId('code-history-load-more')).toHaveCount(0)
      const allPageOids = await history.locator('code').allTextContents()
      expect(new Set(allPageOids).size).toBe(22)
      expect(allPageOids.slice(0, 20)).toEqual(firstPageOids)

      const commitDiffFiles = page.getByTestId('code-commit-diff-files')
      await expect(commitDiffFiles.getByRole('button', { name: 'README.md' })).toBeVisible({
        timeout: 20_000,
      })
      await expect(
        commitDiffFiles.getByRole('button', { name: 'docs/notes with spaces.txt' }),
      ).toBeVisible()
      await commitDiffFiles.getByRole('button', { name: 'docs/notes with spaces.txt' }).click()
      await expect(page.getByTestId('code-commit-diff-document')).toContainText('diff path fixture', {
        timeout: 15_000,
      })

      await expect(page.getByTestId('code-tree')).toBeVisible()
      await expect(page.getByTestId('code-entry-blob').filter({ hasText: 'README.md' })).toBeVisible({
        timeout: 20_000,
      })
      await page.getByTestId('code-entry-blob').filter({ hasText: 'README.md' }).click()
      await expect(page.getByTestId('code-file-body')).toContainText('repo-code-console', {
        timeout: 15_000,
      })
      await expect(page.getByTestId('code-file-body')).toContainText('fixture line')
    } finally {
      await revokePat(page, pat.id)
    }
  })
})
