import { expect, test } from '@playwright/test'

import { goAccountTokens, goAudit, goRunner, loginFromHome } from './helpers/navigation'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
const apiBase = process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'
/** Full PATs are much longer than masked `token_prefix` shown in UI. */
const fullPatLike = /mhp_[a-zA-Z0-9]{24,}/

test.describe('audit and runner status panels', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('audit and runner panels load without exposing PAT secrets', async ({ page }) => {
    await loginFromHome(page, password!)
    await expectLoggedInAs(page, 'admin')

    await goAudit(page)
    await expect(page.getByTestId('audit-panel')).toBeVisible()
    await page.getByTestId('audit-resource-type').selectOption('repository')
    await page.getByTestId('audit-apply-filter').click()
    await expect(page.getByTestId('audit-panel')).toBeVisible()
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
    const login = await request.post(`${apiBase}/api/v1/auth/login`, {
      data: { handle: 'admin', password: password! },
    })
    expect(login.ok()).toBeTruthy()
    const caps = await request.get(`${apiBase}/api/v1/runner/capabilities`)
    expect(caps.ok()).toBeTruthy()
    const json = (await caps.json()) as { pipeline_executor: string; note: string }
    expect(json.pipeline_executor).toMatch(/inprocess|remote/)
    expect(json.note.length).toBeGreaterThan(10)
  })
})
