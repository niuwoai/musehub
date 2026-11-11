import type { ReactNode } from 'react'
import { useParams } from 'react-router-dom'
import { DEFAULT_ORG } from '../app/paths'
import { NotFoundPage } from '../pages/NotFoundPage'

export function OrgScopeGuard({ children }: { children: ReactNode }) {
  const { org } = useParams()
  if (org && org !== DEFAULT_ORG) {
    return <NotFoundPage />
  }
  return <>{children}</>
}
