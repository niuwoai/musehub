import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

export function redactGitSecrets(text: string, pat: string): string {
  if (!text) return text
  let out = text
  if (pat) {
    out = out.split(pat).join('[REDACTED_PAT]')
    const basic = Buffer.from(`git:${pat}`, 'utf8').toString('base64')
    out = out.split(basic).join('[REDACTED_BASIC]')
  }
  out = out.replace(/Authorization:\s*Basic\s+[A-Za-z0-9+/=]+/gi, 'Authorization: Basic [REDACTED]')
  out = out.replace(/mhp_[A-Za-z0-9_-]+/g, '[REDACTED_PAT]')
  return out
}

class GitCommandError extends Error {
  readonly exitCode: number | null
  constructor(message: string, exitCode: number | null) {
    super(message)
    this.name = 'GitCommandError'
    this.exitCode = exitCode
  }
}

/** Ephemeral GIT_CONFIG_GLOBAL: PAT only in mode-0600 file, never git argv. */
function createGitAuthSandbox(pat: string): { dir: string; configPath: string; dispose: () => void } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mh-e2e-gitcfg-'))
  fs.chmodSync(dir, 0o700)
  const configPath = path.join(dir, 'gitconfig')
  const basic = Buffer.from(`git:${pat}`, 'utf8').toString('base64')
  fs.writeFileSync(
    configPath,
    `[credential]\n\thelper =\n[http]\n\textraHeader = Authorization: Basic ${basic}\n`,
    { mode: 0o600 },
  )
  return {
    dir,
    configPath,
    dispose: () => fs.rmSync(dir, { recursive: true, force: true }),
  }
}

function gitEnv(pat: string, configPath: string): NodeJS.ProcessEnv {
  return {
    ...process.env,
    GIT_TERMINAL_PROMPT: '0',
    GIT_ASKPASS: '',
    SSH_ASKPASS: '',
    GIT_CONFIG_GLOBAL: configPath,
    GIT_CONFIG_SYSTEM: '/dev/null',
    GIT_CONFIG_NOSYSTEM: '1',
  }
}

function runGit(
  cwd: string,
  pat: string,
  configPath: string,
  args: string[],
): { stdout: string; stderr: string } {
  const result = spawnSync('git', args, {
    cwd,
    env: gitEnv(pat, configPath),
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  const stdout = result.stdout ?? ''
  const stderr = result.stderr ?? ''
  if (result.error) {
    throw new GitCommandError(
      redactGitSecrets(
        `git ${args.join(' ')}: ${result.error.message} — ${stderr} ${stdout}`,
        pat,
      ),
      null,
    )
  }
  if (result.status !== 0) {
    throw new GitCommandError(
      redactGitSecrets(
        `git ${args.join(' ')} failed (exit ${result.status}): ${stderr} ${stdout}`.trim(),
        pat,
      ),
      result.status,
    )
  }
  return { stdout, stderr }
}

function assertNoSecretLeak(serialized: string, pat: string): void {
  const basic = Buffer.from(`git:${pat}`, 'utf8').toString('base64')
  if (serialized.includes(pat) || serialized.includes(basic) || /mhp_[A-Za-z0-9_-]+/.test(serialized)) {
    throw new Error('git test helper would have leaked credential material')
  }
}

/**
 * Induces a clone failure and asserts PAT / Basic material never appears in errors or repo config.
 * Used by vitest regression (no live server required).
 */
export function verifyGitAuthDoesNotLeakOnFailure(pat: string): void {
  const fakePat = pat || 'mhp_fake_leak_probe_abcdefghijklmnopqrst'
  const sandbox = createGitAuthSandbox(fakePat)
  const workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mh-e2e-git-leak-'))
  let caught: unknown
  try {
    runGit(workDir, fakePat, sandbox.configPath, [
      'clone',
      'http://127.0.0.1:9/musehub-e2e-void.git',
      '.',
    ])
  } catch (e) {
    caught = e
  } finally {
    sandbox.dispose()
  }
  if (!caught) {
    throw new Error('expected git clone to fail for leak probe')
  }
  const serialized =
    JSON.stringify(caught) +
    (caught instanceof Error ? caught.message + caught.stack : String(caught))
  assertNoSecretLeak(serialized, fakePat)

  const gitConfig = path.join(workDir, '.git', 'config')
  if (fs.existsSync(gitConfig)) {
    const configBody = fs.readFileSync(gitConfig, 'utf8')
    assertNoSecretLeak(configBody, fakePat)
    expectCredentialFreeRemote(configBody)
  }
  fs.rmSync(workDir, { recursive: true, force: true })
}

function expectCredentialFreeRemote(configBody: string): void {
  if (/@/.test(configBody) && /mhp_/.test(configBody)) {
    throw new Error('git config contains credential-bearing remote')
  }
}

/** Authenticated clone/push against smart HTTP using a PAT (never embed token in remote URL or argv). */
export function gitPushBranch(
  cloneUrl: string,
  pat: string,
  branch: string,
  files: Record<string, string>,
): void {
  const sandbox = createGitAuthSandbox(pat)
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mh-e2e-git-'))
  try {
    const url = cloneUrl.replace(/\/\/[^@]+@/, '//')
    runGit(dir, pat, sandbox.configPath, ['clone', url, '.'])
    try {
      runGit(dir, pat, sandbox.configPath, ['checkout', branch])
    } catch {
      runGit(dir, pat, sandbox.configPath, ['checkout', '-b', branch])
    }
    for (const [file, body] of Object.entries(files)) {
      const full = path.join(dir, file)
      fs.mkdirSync(path.dirname(full), { recursive: true })
      fs.writeFileSync(full, body)
    }
    runGit(dir, pat, sandbox.configPath, ['add', '.'])
    runGit(dir, pat, sandbox.configPath, [
      '-c',
      'user.email=e2e@musehub.local',
      '-c',
      'user.name=musehub-e2e',
      'commit',
      '-m',
      'e2e',
    ])
    runGit(dir, pat, sandbox.configPath, ['push', 'origin', `HEAD:${branch}`])
    runGit(dir, pat, sandbox.configPath, ['remote', 'set-url', 'origin', url])
    const configBody = fs.readFileSync(path.join(dir, '.git', 'config'), 'utf8')
    expectCredentialFreeRemote(configBody)
    if (configBody.includes(pat)) {
      throw new Error('PAT persisted in git config')
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
    sandbox.dispose()
  }
}
