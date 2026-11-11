import { describe, expect, it } from 'vitest'

import { redactGitSecrets, verifyGitAuthDoesNotLeakOnFailure } from './s1-git-seed'

describe('s1-git-seed credential hygiene', () => {
  const pat = 'mhp_leak_regression_probe_token_value'

  it('redactGitSecrets strips PAT, Basic payload, and mhp_ literals', () => {
    const basic = Buffer.from(`git:${pat}`, 'utf8').toString('base64')
    const raw = `argv -c http.extraHeader=Authorization: Basic ${basic} err ${pat}`
    const redacted = redactGitSecrets(raw, pat)
    expect(redacted).not.toContain(pat)
    expect(redacted).not.toContain(basic)
    expect(redacted).toContain('[REDACTED')
  })

  it('verifyGitAuthDoesNotLeakOnFailure: failed clone has no secret in error or config', () => {
    expect(() => verifyGitAuthDoesNotLeakOnFailure(pat)).not.toThrow()
  })
})
