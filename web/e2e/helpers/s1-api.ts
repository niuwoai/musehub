import type { Page } from '@playwright/test'
import { e2eBrowserApiOrigin } from './origin'
import { expectResponseOk } from './repo'

/** Session-authenticated JSON API (Vite proxy origin). */
export const apiBase = () => e2eBrowserApiOrigin()

export async function cookieHeader(page: Page): Promise<Record<string, string>> {
  const cookies = await page.context().cookies()
  const cookie = cookies.map((c) => `${c.name}=${c.value}`).join('; ')
  return cookie ? { Cookie: cookie } : {}
}

export async function apiRaw(
  page: Page,
  method: string,
  path: string,
  body?: unknown,
) {
  return page.request.fetch(`${apiBase()}${path}`, {
    method,
    headers: {
      ...(await cookieHeader(page)),
      'Content-Type': 'application/json',
    },
    data: body,
  })
}

export async function apiJson<T>(
  page: Page,
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const headers: Record<string, string> = {
    ...(await cookieHeader(page)),
    'Content-Type': 'application/json',
  }
  const r = await page.request.fetch(`${apiBase()}${path}`, {
    method,
    headers,
    data: body,
  })
  await expectResponseOk(r, `${method} ${path}`)
  if (r.status() === 204) return undefined as T
  const text = await r.text()
  if (!text.trim()) return undefined as T
  return JSON.parse(text) as T
}
