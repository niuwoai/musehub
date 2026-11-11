import { execSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

export function gitPushBranch(
  cloneUrl: string,
  branch: string,
  files: Record<string, string>,
): void {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mh-e2e-git-'))
  execSync(`git clone "${cloneUrl}" "${dir}"`, { stdio: 'pipe' })
  try {
    execSync(`git checkout ${branch}`, { cwd: dir, stdio: 'pipe' })
  } catch {
    execSync(`git checkout -b ${branch}`, { cwd: dir, stdio: 'pipe' })
  }
  for (const [file, body] of Object.entries(files)) {
    const full = path.join(dir, file)
    fs.mkdirSync(path.dirname(full), { recursive: true })
    fs.writeFileSync(full, body)
  }
  execSync('git add .', { cwd: dir, stdio: 'pipe' })
  execSync(
    'git -c user.email=e2e@musehub.local -c user.name=musehub-e2e commit -m e2e',
    { cwd: dir, stdio: 'pipe' },
  )
  execSync(`git push origin HEAD:${branch}`, { cwd: dir, stdio: 'pipe' })
}
