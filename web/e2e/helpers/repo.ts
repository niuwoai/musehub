import { expect, type Locator, type Page, type Response } from '@playwright/test'
import { e2eBrowserApiOrigin } from './origin'
import { waitForSessionAuthSettled } from './session'

function repoWorkspaceListGet(response: Response, slug: string): boolean {
  if (response.request().method() !== 'GET' || !response.ok()) {
    return false
  }
  const url = response.url()
  if (!url.includes(`/repos/${slug}/`)) {
    return false
  }
  return (
    url.includes('/issues') ||
    url.includes('/pull-requests') ||
    url.includes('/deployments') ||
    url.includes('/releases')
  )
}

const apiBase = () => e2eBrowserApiOrigin()

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

async function readRepoNavSlugSet(page: Page): Promise<Set<string>> {
  const texts = await repoNavButtons(page).allInnerTexts()
  return new Set(texts.map((t) => t.trim()).filter((t) => t.length > 0))
}

/** Align sidebar repo buttons with GET /orgs/demo/repos (count + slugs). */
async function expectRepoNavMatchesApi(page: Page): Promise<void> {
  const nav = repoNavButtons(page)
  await expect
    .poll(
      async () => {
        const repos = await fetchDemoReposViaPage(page)
        const navSlugs = await readRepoNavSlugSet(page)
        if (repos.length === 0) {
          return navSlugs.size === 0
        }
        if (navSlugs.size !== repos.length) {
          return false
        }
        for (const { slug } of repos) {
          if (!navSlugs.has(slug)) {
            return false
          }
        }
        return true
      },
      { timeout: 30_000 },
    )
    .toBe(true)
  await expect(nav).toHaveCount(await fetchDemoReposViaPage(page).then((r) => r.length), {
    timeout: 5_000,
  })
}

export async function waitForDemoRepoNavHydrated(page: Page): Promise<void> {
  await waitForSessionAuthSettled(page)
  await expect(page.getByRole('button', { name: /create repo|创建仓库/i })).toBeVisible({
    timeout: 15_000,
  })

  const onReposList = /^\/org\/demo\/repos\/?$/.test(new URL(page.url()).pathname)
  const listPromise = page.waitForResponse((r) => demoReposListGet(r), {
    timeout: 30_000,
  })
  if (onReposList) {
    // Login lands on the list before `refreshRepos()` finishes; leave and re-enter to bind list GET.
    await page.getByTestId('nav-org-settings').click()
    await expect(page).toHaveURL(/\/org\/demo\/settings/, { timeout: 15_000 })
  }
  await page.getByTestId('nav-repos').click()
  await expect(page).toHaveURL(/\/org\/demo\/repos\/?$/, { timeout: 15_000 })
  const captured = await listPromise
  await expectResponseOk(captured, 'list demo repos (nav open)')
  await expectRepoNavMatchesApi(page)
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
  await expect(page.getByTestId('create-issue')).toBeVisible({ timeout: 30_000 })
  await ensureRepoListPage(page)
  const repoEntry = repoNavButtonForSlug(page, slug)
  await expect(repoEntry).toHaveCount(1, { timeout: 15_000 })
  return { slug, repoEntry }
}

export async function openRepoDelivery(page: Page, slug: string): Promise<void> {
  await waitForSessionAuthSettled(page)
  const repoContextReady = page.waitForResponse((r) => repoWorkspaceListGet(r, slug), {
    timeout: 30_000,
  })
  await page.goto(`/org/demo/repos/${slug}/delivery`, { waitUntil: 'domcontentloaded' })
  try {
    await repoContextReady
  } catch {
    await openRepoBySlug(page, slug)
    const deliveryReady = page.waitForResponse((r) => repoWorkspaceListGet(r, slug), {
      timeout: 30_000,
    })
    await page.getByTestId('nav-repo-delivery').click()
    await expect(page).toHaveURL(new RegExp(`/repos/${slug}/delivery`), { timeout: 15_000 })
    await deliveryReady
  }
  await expectRepoSlugInSubnav(page, slug)
  await expect(page.getByTestId('delivery-panel')).toBeVisible({ timeout: 30_000 })
}

export async function expectRepoSlugInSubnav(page: Page, slug: string): Promise<void> {
  await expect(page.getByTestId('repo-subnav-title')).toHaveText(slug, { timeout: 15_000 })
}

export async function openRepoSettings(page: Page, slug: string): Promise<void> {
  const repoContextReady = page.waitForResponse(
    (r) =>
      r.request().method() === 'GET' &&
      r.ok() &&
      r.url().includes(`/repos/${slug}/`) &&
      (r.url().includes('/issues') ||
        r.url().includes('/pull-requests') ||
        r.url().includes('/environments')),
    { timeout: 30_000 },
  )
  await page.goto(`/org/demo/repos/${slug}/settings`, { waitUntil: 'domcontentloaded' })
  await waitForSessionAuthSettled(page)
  await repoContextReady
  await expectRepoSlugInSubnav(page, slug)
  await expect(page.getByTestId('repo-settings-panel')).toBeVisible({ timeout: 30_000 })
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
  await expect(page.getByTestId('create-issue')).toBeVisible({ timeout: 30_000 })
}
