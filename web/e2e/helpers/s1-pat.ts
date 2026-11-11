import type { Page } from '@playwright/test'
import { apiBase, cookieHeader } from './s1-api'

export type MintedPat = { id: string; token: string }

/** Mint a repo-scoped PAT for Git smart HTTP. Caller must revoke when done. */
export async function mintGitPat(
  page: Page,
  name: string,
  repoId: string,
): Promise<MintedPat> {
  const headers = {
    ...(await cookieHeader(page)),
    'Content-Type': 'application/json',
  }
  const r = await page.request.post(`${apiBase()}/api/v1/tokens`, {
    headers,
    data: {
      name,
      org: 'demo',
      scopes: ['repo:read', 'repo:write'],
      scope_mode: 'selected',
      repo_ids: [repoId],
    },
  })
  if (!r.ok()) {
    throw new Error(`mint git PAT failed: HTTP ${r.status()}`)
  }
  const body = (await r.json()) as { id: string; token: string }
  if (!body.token?.startsWith('mhp_')) {
    throw new Error('mint git PAT: missing token')
  }
  return { id: body.id, token: body.token }
}

export async function revokePat(page: Page, tokenId: string): Promise<void> {
  const r = await page.request.delete(`${apiBase()}/api/v1/tokens/${tokenId}`, {
    headers: await cookieHeader(page),
  })
  if (!r.ok() && r.status() !== 404) {
    throw new Error(`revoke PAT failed: HTTP ${r.status()}`)
  }
}
