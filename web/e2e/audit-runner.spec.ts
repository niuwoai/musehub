import { expect, test } from '@playwright/test'

import { goAccountTokens, goAudit, goRunner, loginFromHome } from './helpers/navigation'
import { expectLoggedInAs } from './helpers/session'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('audit and runner panels', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('audit, runner, and PAT are reachable via nav', async ({ page }) => {
    await loginFromHome(page, password!)
    await expectLoggedInAs(page, 'admin')

    await goAudit(page)
    await expect(page.getByTestId('audit-panel')).toBeVisible()
    await goRunner(page)
    await expect(page.getByTestId('runner-status-panel')).toBeVisible()
    await expect(page.getByTestId('runner-executors')).toBeVisible()
    await goAccountTokens(page)
    await expect(page.getByTestId('pat-management')).toBeVisible()

    await goAudit(page)
    await expect(page.getByTestId('audit-panel')).toBeVisible()
  })
})
