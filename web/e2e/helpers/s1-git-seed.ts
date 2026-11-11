import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

function basicAuthHeader(user: string, secret: string): string {
  return `Authorization: Basic ${Buffer.from(`${user}:${secret}`, 'utf8').toString('base64')}`
}

function gitConfigArgs(pat: string): string[] {
  return [
    '-c',
    'credential.helper=',
    '-c',
    `http.extraHeader=${basicAuthHeader('git', pat)}`,
  ]
}

function runGit(cwd: string, pat: string, args: string[]): void {
  execFileSync('git', [...gitConfigArgs(pat), ...args], {
    cwd,
    env: { ...process.env, GIT_TERMINAL_PROMPT: '0' },
    stdio: 'pipe',
  })
}

/** Authenticated clone/push against smart HTTP using a PAT (never embed token in remote URL). */
export function gitPushBranch(
  cloneUrl: string,
  pat: string,
  branch: string,
  files: Record<string, string>,
): void {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mh-e2e-git-'))
  try {
    const url = cloneUrl.replace(/\/\/[^@]+@/, '//')
    runGit(dir, pat, ['clone', url, '.'])
    try {
      runGit(dir, pat, ['checkout', branch])
    } catch {
      runGit(dir, pat, ['checkout', '-b', branch])
    }
    for (const [file, body] of Object.entries(files)) {
      const full = path.join(dir, file)
      fs.mkdirSync(path.dirname(full), { recursive: true })
      fs.writeFileSync(full, body)
    }
    runGit(dir, pat, ['add', '.'])
    execFileSync(
      'git',
      [
        ...gitConfigArgs(pat),
        '-c',
        'user.email=e2e@musehub.local',
        '-c',
        'user.name=musehub-e2e',
        'commit',
        '-m',
        'e2e',
      ],
      { cwd: dir, env: { ...process.env, GIT_TERMINAL_PROMPT: '0' }, stdio: 'pipe' },
    )
    runGit(dir, pat, ['push', 'origin', `HEAD:${branch}`])
    runGit(dir, pat, ['remote', 'set-url', 'origin', url])
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
}
