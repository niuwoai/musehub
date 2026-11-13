import { expect, type Page } from '@playwright/test'

const SESSION_COOKIE = 'musehub_session'

/** Stable post-login assertion (avoids ambiguous `getByText(/admin/i)` from org role labels). */
export async function expectLoggedInAs(page: Page, handle: string) {
  await expect(page.getByTestId('session-handle')).toHaveText(handle, { timeout: 15_000 })
  await expectSessionCookie(page)
}

/** Session cookie must exist before `page.request` hits the Vite-proxied API. */
export async function expectSessionCookie(page: Page) {
  await expect
    .poll(
      async () => {
        const cookies = await page.context().cookies()
        return cookies.some((c) => c.name === SESSION_COOKIE && c.value.length > 0)
      },
      { timeout: 15_000 },
    )
    .toBe(true)
}

export async function loginAsAdmin(
  page: Page,
  password: string,
  handle = 'admin',
): Promise<void> {
  await page.goto('/login', { waitUntil: 'domcontentloaded' })
  await page.getByTestId('login-handle-input').fill(handle)
  await page.getByLabel(/password|密码/i).fill(password)
  await page.getByRole('button', { name: /login|登录/i }).click()
  await expectLoggedInAs(page, handle)
}
