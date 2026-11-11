import { describe, expect, it } from 'vitest'
import { resolveSafeRedirectPath } from './authRedirect'
import { paths } from './paths'

describe('resolveSafeRedirectPath', () => {
  it('defaults when from is missing', () => {
    expect(resolveSafeRedirectPath(null)).toBe(paths.repos('demo'))
  })

  it('preserves deep link under /org/demo', () => {
    expect(
      resolveSafeRedirectPath({
        pathname: '/org/demo/repos/foo/delivery',
        search: '',
        hash: '',
      }),
    ).toBe('/org/demo/repos/foo/delivery')
  })

  it('rejects protocol-relative paths', () => {
    expect(
      resolveSafeRedirectPath({
        pathname: '//evil.test/phish',
        search: '',
        hash: '',
      }),
    ).toBe(paths.repos('demo'))
  })

  it('rejects login as target to avoid loops', () => {
    expect(
      resolveSafeRedirectPath({
        pathname: '/login',
        search: '?x=1',
        hash: '',
      }),
    ).toBe(paths.repos('demo'))
  })
})
