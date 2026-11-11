import { describe, expect, it } from 'vitest'

import {
  demoReposCreatePost,
  demoReposListGet,
  demoReposPath,
} from './repo'

describe('demoReposPath', () => {
  it('matches list and create URLs on the demo org', () => {
    expect(demoReposPath('http://127.0.0.1:5173/api/v1/orgs/demo/repos')).toBe(true)
    expect(demoReposPath('http://127.0.0.1:8080/api/v1/orgs/demo/repos')).toBe(true)
  })

  it('rejects nested repo resource paths', () => {
    expect(demoReposPath('http://127.0.0.1:8080/api/v1/orgs/demo/repos/my-repo/issues')).toBe(
      false,
    )
    expect(demoReposPath('http://127.0.0.1:8080/api/v1/orgs/other/repos')).toBe(false)
  })
})

describe('demoRepos response matchers', () => {
  const listUrl = 'http://127.0.0.1:8080/api/v1/orgs/demo/repos'

  it('distinguishes GET list from POST create', () => {
    const getResp = { url: () => listUrl, request: () => ({ method: () => 'GET' }) }
    const postResp = { url: () => listUrl, request: () => ({ method: () => 'POST' }) }
    expect(demoReposListGet(getResp as never)).toBe(true)
    expect(demoReposCreatePost(postResp as never)).toBe(true)
    expect(demoReposListGet(postResp as never)).toBe(false)
    expect(demoReposCreatePost(getResp as never)).toBe(false)
  })
})
