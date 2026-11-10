import { expect, type Page } from '@playwright/test'

/** Stable post-login assertion (avoids ambiguous `getByText(/admin/i)` from org role labels). */
export async function expectLoggedInAs(page: Page, handle: string) {
  await expect(page.getByTestId('session-handle')).toHaveText(handle, { timeout: 15_000 })
}
