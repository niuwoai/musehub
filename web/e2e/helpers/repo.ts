import { expect, type Locator, type Page } from '@playwright/test'

export function repoNavButtons(page: Page) {
  return page.locator('section > ul').first().getByRole('button')
}

function demoReposPath(url: string): boolean {
  try {
    return new URL(url).pathname === '/api/v1/orgs/demo/repos'
  } catch {
    return false
  }
}

/** Creates a repo via the console and returns the new list entry (last in nav). */
export async function createRepoViaUi(page: Page): Promise<{
  slug: string
  repoEntry: Locator
}> {
  const repoCountBefore = await repoNavButtons(page).count()
  const [createResp] = await Promise.all([
    page.waitForResponse(
      (r) => demoReposPath(r.url()) && r.request().method() === 'POST' && r.ok(),
      { timeout: 60_000 },
    ),
    page.getByRole('button', { name: /create repo|创建仓库/i }).click(),
  ])
  expect(createResp.ok()).toBeTruthy()
  await page.waitForResponse(
    (r) => demoReposPath(r.url()) && r.request().method() === 'GET' && r.ok(),
    { timeout: 60_000 },
  )
  await expect(repoNavButtons(page)).toHaveCount(repoCountBefore + 1, { timeout: 60_000 })
  const repoEntry = repoNavButtons(page).last()
  const slug = (await repoEntry.textContent())?.trim() ?? ''
  return { slug, repoEntry }
}
