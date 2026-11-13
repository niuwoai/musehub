import { expect, test, type Page, type Response } from '@playwright/test'

import { e2eServerOrigin } from './helpers/origin'
import { goAccountTokens, goAudit, goRunner, loginFromHome } from './helpers/navigation'
import { expectLoggedInAs } from './helpers/session'
import { apiBase, apiJson } from './helpers/s1-api'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
/** Full PATs are much longer than masked `token_prefix` shown in UI. */
const fullPatLike = /mhp_[a-zA-Z0-9]{24,}/

function repositoryAuditListResponse(r: Response) {
  return (
    r.url().includes('/api/v1/orgs/demo/audit/events') &&
    r.request().method() === 'GET' &&
    r.url().includes('resource_type=repository') &&
    r.ok()
  )
}

function captureRepositoryAuditList(page: Page) {
  return page.waitForResponse(repositoryAuditListResponse, { timeout: 30_000 }).then(async (resp) => {
    const items = (await resp.json()) as { resource_type: string }[]
    expect(items.length).toBeGreaterThan(0)
    items.forEach((ev) => expect(ev.resource_type).toBe('repository'))
    return items
  })
}

/** Select fires useEffect reload; Apply fires a second GET — await both before DOM asserts. */
async function applyRepositoryAuditFilter(page: Page) {
  await expect(page.getByTestId('audit-apply-filter')).toBeVisible()
  const afterSelect = captureRepositoryAuditList(page)
  await page.getByTestId('audit-resource-type').selectOption('repository')
  await afterSelect
  const afterApply = captureRepositoryAuditList(page)
  await page.getByTestId('audit-apply-filter').click()
  await afterApply
  const auditRows = page.getByTestId('audit-event')
  const resources = page.getByTestId('audit-event-resource')
  await expect(auditRows.first()).toBeVisible({ timeout: 15_000 })
  await expect(resources.first()).toBeVisible({ timeout: 15_000 })
}

test.describe('audit and runner status panels', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('audit and runner panels load without exposing PAT secrets', async ({ page }) => {
    await loginFromHome(page, password!)
    await expectLoggedInAs(page, 'admin')

    const repoSlug = `audit-e2e-${Date.now()}`
    await apiJson(page, 'POST', '/api/v1/orgs/demo/repos', {
      slug: repoSlug,
      description: 'audit filter e2e',
    })
    const seeded = await page.request.get(
      `${apiBase()}/api/v1/orgs/demo/audit/events?resource_type=repository&limit=40`,
    )
    await expect(seeded).toBeOK()
    const seededEvents = (await seeded.json()) as { action: string; metadata?: { slug?: string } }[]
    expect(
      seededEvents.some(
        (ev) => ev.action === 'repository.create' && ev.metadata?.slug === repoSlug,
      ),
    ).toBeTruthy()

    await goAudit(page)
    await applyRepositoryAuditFilter(page)
    const auditRows = page.getByTestId('audit-event')
    const n = await auditRows.count()
    expect(n).toBeGreaterThan(0)
    for (let i = 0; i < n; i++) {
      await expect(auditRows.nth(i).getByTestId('audit-event-resource')).toHaveText(
        /^repository\//,
      )
    }
    await goRunner(page)
    await expect(page.getByTestId('runner-status-panel')).toBeVisible()
    await expect(page.getByTestId('runner-executors')).toBeVisible()
    await goAccountTokens(page)
    await expect(page.getByTestId('pat-management')).toBeVisible()

    const bodyText = await page.locator('body').innerText()
    expect(bodyText).not.toMatch(fullPatLike)

    await page.reload()
    await expectLoggedInAs(page, 'admin')
    await goAudit(page)
    await expect(page.getByTestId('audit-panel')).toBeVisible()
    expect(await page.locator('body').innerText()).not.toMatch(fullPatLike)
  })

  test('runner capabilities API is readable with session', async ({ request }) => {
    const login = await request.post(`${e2eServerOrigin()}/api/v1/auth/login`, {
      data: { handle: 'admin', password: password! },
    })
    expect(login.ok()).toBeTruthy()
    const caps = await request.get(`${e2eServerOrigin()}/api/v1/runner/capabilities`)
    expect(caps.ok()).toBeTruthy()
    const json = (await caps.json()) as { pipeline_executor: string; note: string }
    expect(json.pipeline_executor).toMatch(/inprocess|remote/)
    expect(json.note.length).toBeGreaterThan(10)
  })
})
