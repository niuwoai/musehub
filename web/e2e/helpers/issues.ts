import { expect, type Page, type Response } from '@playwright/test'

export async function fillIssueCreateForm(
  page: Page,
  title: string,
  assigneeHandle?: string,
): Promise<void> {
  await page.getByTestId('issue-create-title').fill(title)
  if (assigneeHandle !== undefined && assigneeHandle.length > 0) {
    await page.getByTestId('issue-create-assignee').selectOption(assigneeHandle)
  }
}

function issueCreatePost(response: Response, slug?: string): boolean {
  if (response.request().method() !== 'POST' || !response.ok()) {
    return false
  }
  const url = response.url()
  if (!url.includes('/issues') || /\/issues\/\d+/.test(url)) {
    return false
  }
  if (slug && !url.includes(`/repos/${slug}/`)) {
    return false
  }
  return true
}

/** Clicks create after filling title; waits for successful issue POST. */
export async function createIssueViaUi(
  page: Page,
  title: string,
  opts?: { assigneeHandle?: string; slug?: string },
): Promise<Response> {
  await fillIssueCreateForm(page, title, opts?.assigneeHandle)
  const post = page.waitForResponse((r) => issueCreatePost(r, opts?.slug), {
    timeout: 30_000,
  })
  await expect(page.getByTestId('create-issue')).toBeEnabled()
  await page.getByTestId('create-issue').click()
  return post
}
