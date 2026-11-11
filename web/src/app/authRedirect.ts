import type { Location } from 'react-router-dom'
import { DEFAULT_ORG, paths } from './paths'

export type RedirectFrom = Pick<Location, 'pathname' | 'search' | 'hash'>

/** Allow only same-origin relative app paths (no open redirects). */
export function resolveSafeRedirectPath(from: RedirectFrom | undefined | null): string {
  const pathname = from?.pathname?.trim()
  if (!pathname || pathname === '/' || pathname === paths.login()) {
    return paths.repos(DEFAULT_ORG)
  }
  if (!pathname.startsWith('/') || pathname.startsWith('//')) {
    return paths.repos(DEFAULT_ORG)
  }
  if (pathname.includes('://')) {
    return paths.repos(DEFAULT_ORG)
  }
  const search = from?.search ?? ''
  const hash = from?.hash ?? ''
  return `${pathname}${search}${hash}`
}
