import type { Page } from '@playwright/test'
import { expectResponseOk } from './repo'

const apiBase = () => process.env.MUSEHUB_E2E_API ?? 'http://127.0.0.1:8080'

async function cookieHeader(page: Page): Promise<Record<string, string>> {
  const cookies = await page.context().cookies()
  const cookie = cookies.map((c) => `${c.name}=${c.value}`).join('; ')
  return cookie ? { Cookie: cookie } : {}
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
  return (await r.json()) as T
}
