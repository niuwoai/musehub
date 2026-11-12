import { expect, type Locator, type Page, type Response } from '@playwright/test'

const apiBase = () => process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

export function repoNavButtons(page: Page) {
  return page.getByTestId('repo-list').getByRole('button')
}

/** @internal exported for unit tests */
export function demoReposPath(url: string): boolean {
  try {
    return new URL(url).pathname === '/api/v1/orgs/demo/repos'
  } catch {
    return false
  }
}

export function demoReposListGet(response: Response): boolean {
  return demoReposPath(response.url()) && response.request().method() === 'GET'
}

export function demoReposCreatePost(response: Response): boolean {
  return demoReposPath(response.url()) && response.request().method() === 'POST'
}

export async function expectResponseOk(response: Response, label: string): Promise<void> {
  if (response.ok()) {
    return
  }
  const body = await response.text().catch(() => '')
  throw new Error(
    `${label}: HTTP ${response.status()} ${response.statusText()} — ${body.slice(0, 240)}`,
  )
}

async function fetchDemoReposViaPage(page: Page): Promise<{ slug: string }[]> {
  const cookies = await page.context().cookies()
  const cookie = cookies.map((c) => `${c.name}=${c.value}`).join('; ')
  const r = await page.request.get(`${apiBase()}/api/v1/orgs/demo/repos`, {
    headers: cookie ? { Cookie: cookie } : {},
  })
  await expectResponseOk(r, 'list demo repos via API')
  return (await r.json()) as { slug: string }[]
}

/**
 * After login, the repo nav can lag behind `refreshRepos()` until the list GET completes.
 * Align the nav with the API before counting or picking `.last()`.
 */
export async function ensureRepoListPage(page: Page): Promise<void> {
  const path = new URL(page.url()).pathname
  if (!/^\/org\/demo\/repos\/?$/.test(path)) {
    await page.getByTestId('nav-repos').click()
    await expect(page).toHaveURL(/\/org\/demo\/repos\/?$/, { timeout: 15_000 })
  }
  await waitForDemoRepoNavHydrated(page)
}

export async function waitForDemoRepoNavHydrated(page: Page): Promise<void> {
  await expect(page.getByRole('button', { name: /create repo|创建仓库/i })).toBeVisible({
    timeout: 15_000,
  })
  const nav = repoNavButtons(page)

  const inFlightList = page
    .waitForResponse((r) => demoReposListGet(r), { timeout: 5_000 })
    .catch(() => null)

  let repos: { slug: string }[]
  const captured = await inFlightList
  if (captured) {
    await expectResponseOk(captured, 'list demo repos (in-flight after login)')
    repos = (await captured.json()) as { slug: string }[]
  } else {
    repos = await fetchDemoReposViaPage(page)
  }

  if (repos.length === 0) {
    await expect(nav).toHaveCount(0, { timeout: 5_000 })
    return
  }

  await expect(nav).toHaveCount(repos.length, { timeout: 15_000 })
  for (const { slug } of repos) {
    await expect(nav.filter({ hasText: slug })).toHaveCount(1, { timeout: 5_000 })
  }
}

export function repoNavButtonForSlug(page: Page, slug: string): Locator {
  return repoNavButtons(page).filter({ hasText: slug })
}

/** Creates a repo via the console; selects the new repo by POST response slug. */
export async function createRepoViaUi(page: Page): Promise<{
  slug: string
  repoEntry: Locator
}> {
  await ensureRepoListPage(page)

  let createPostSettled = false
  const createWait = page.waitForResponse(
    (r) => {
      if (!demoReposCreatePost(r)) {
        return false
      }
      createPostSettled = true
      return true
    },
    { timeout: 60_000 },
  )
  const listWait = page.waitForResponse(
    (r) => demoReposListGet(r) && createPostSettled,
    { timeout: 60_000 },
  )
  await page.getByRole('button', { name: /create repo|创建仓库/i }).click()
  const [createResp, listResp] = await Promise.all([createWait, listWait])
  await expectResponseOk(createResp, 'create demo repo')
  await expectResponseOk(listResp, 'refresh demo repos after create')

  const { slug } = (await createResp.json()) as { slug: string }
  expect(slug.length).toBeGreaterThan(0)
  const listed = (await listResp.json()) as { slug: string }[]
  expect(listed.some((r) => r.slug === slug)).toBe(true)

  await expect(page).toHaveURL(new RegExp(`/org/demo/repos/${slug}/issues`), { timeout: 15_000 })
  await ensureRepoListPage(page)
  const repoEntry = repoNavButtonForSlug(page, slug)
  await expect(repoEntry).toHaveCount(1, { timeout: 15_000 })
  return { slug, repoEntry }
}

export async function openRepoDelivery(page: Page, slug: string): Promise<void> {
  const deliveryLoaded = page.waitForResponse(
    (r) => r.url().includes(`/repos/${slug}/`) && r.request().method() === 'GET',
  )
  await page.goto(`/org/demo/repos/${slug}/delivery`)
  await deliveryLoaded.catch(() => null)
  await expect(page.getByTestId('delivery-panel')).toBeVisible({ timeout: 30_000 })
}

export async function expectRepoSlugInSubnav(page: Page, slug: string): Promise<void> {
  await expect(page.getByTestId('repo-subnav-title')).toHaveText(slug, { timeout: 15_000 })
}

export async function openRepoBySlug(page: Page, slug: string): Promise<void> {
  const issuesLoaded = page.waitForResponse(
    (r) => r.url().includes(`/repos/${slug}/issues`) && r.request().method() === 'GET',
  )
  await page.goto(`/org/demo/repos/${slug}/issues`)
  const issuesResp = await issuesLoaded
  await expectResponseOk(issuesResp, `load issues for repo ${slug}`)
  await expect(page.getByTestId('nav-repo-issues')).toBeVisible({
    timeout: 15_000,
  })
  await expectRepoSlugInSubnav(page, slug)
}
