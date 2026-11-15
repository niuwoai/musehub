import { expect, test } from '@playwright/test'

import { createIssueViaUi } from './helpers/issues'
import { createRepoViaUi, waitForDemoRepoNavHydrated } from './helpers/repo'
import { loginAsAdmin } from './helpers/session'
import { apiJson } from './helpers/s1-api'

const password = process.env.MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD

test.describe('repo issues create', () => {
  test.skip(!password, 'requires bootstrap password and API on :8080')

  test('create with assignee persists on list', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    const member = `e2e-issue-create-${Date.now()}`
    await apiJson(page, 'POST', '/api/v1/orgs/demo/members', {
      handle: member,
      password: 'E2e-Issue-Create-12!',
      role: 'member',
    })
    await apiJson(page, 'POST', `/api/v1/orgs/demo/repos/${slug}/collaborators`, {
      handle: member,
      role: 'maintainer',
    })
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await createIssueViaUi(page, 'On-call rotation', {
      slug,
      assigneeHandle: member,
    })
    await expect(page.getByTestId('issue-assignee-1')).toContainText(`@${member}`)
  })

  test('empty title keeps create disabled', async ({ page }) => {
    await loginAsAdmin(page, password!)
    await waitForDemoRepoNavHydrated(page)
    const { slug } = await createRepoViaUi(page)
    await page.goto(`/org/demo/repos/${slug}/issues`)
    await expect(page.getByTestId('create-issue')).toBeDisabled()
    await page.getByTestId('issue-create-title').fill('   ')
    await expect(page.getByTestId('create-issue')).toBeDisabled()
  })
})
