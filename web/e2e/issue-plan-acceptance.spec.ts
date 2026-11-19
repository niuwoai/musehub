import { expect, test } from '@playwright/test'

import { createIssueViaUi } from './helpers/issues'
import { loginAsAdmin } from './helpers/session'
import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'

const bootstrapPassword = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('Issue implementation plan acceptance', () => {
  test.skip(!bootstrapPassword, 'requires bootstrap admin credentials and API on :8080')

  test('requires the current plan to be accepted before starting an AgentRun', async ({ page }) => {
    await loginAsAdmin(page, bootstrapPassword!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    const issueCreated = await createIssueViaUi(page, 'plan acceptance regression', { slug })
    expect(issueCreated.ok()).toBeTruthy()
    await page.getByTestId('issue-link-1').click()

    const planInput = page.getByTestId('issue-plan-input')
    const acceptPlan = page.getByTestId('issue-accept-plan')
    const startAgent = page.getByTestId('issue-run-agent')
    await expect(planInput).toBeVisible()
    await expect(startAgent).toBeDisabled()

    await planInput.fill('Plan revision one')
    const firstPlanSaved = page.waitForResponse(
      (response) => response.url().includes(`/repos/${slug}/issues/`) &&
        response.url().includes('/plans') && response.request().method() === 'POST',
    )
    const firstPlanAccepted = page.waitForResponse(
      (response) => response.url().includes(`/repos/${slug}/issues/`) &&
        response.url().includes('/plans') && response.request().method() === 'PATCH',
    )
    await page.keyboard.press('Tab')
    await expect(acceptPlan).toBeFocused()
    await page.keyboard.press('Enter')
    expect((await firstPlanSaved).ok()).toBeTruthy()
    expect((await firstPlanAccepted).ok()).toBeTruthy()
    await expect(page.getByTestId('issue-plan-state')).toContainText('accepted')
    await expect(startAgent).toBeEnabled()
    await expect(startAgent).toBeFocused()

    await planInput.fill('Plan revision two')
    await expect(startAgent).toBeDisabled()
    const secondPlanSaved = page.waitForResponse(
      (response) => response.url().includes(`/repos/${slug}/issues/`) &&
        response.url().includes('/plans') && response.request().method() === 'POST',
    )
    const secondPlanAccepted = page.waitForResponse(
      (response) => response.url().includes(`/repos/${slug}/issues/`) &&
        response.url().includes('/plans') && response.request().method() === 'PATCH',
    )
    await acceptPlan.click()
    expect((await secondPlanSaved).ok()).toBeTruthy()
    expect((await secondPlanAccepted).ok()).toBeTruthy()
    await expect(page.getByTestId('issue-plan-state')).toContainText('v2 accepted')
    await expect(startAgent).toBeEnabled()
  })
})
