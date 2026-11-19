import type { ReactNode } from 'react'
import { useParams } from 'react-router-dom'
import { useWorkspace } from '../app/WorkspaceProvider'
import { NotFoundPage } from '../pages/NotFoundPage'

export function OrgScopeGuard({ children }: { children: ReactNode }) {
  const { org } = useParams()
  const { t, organizations, organizationsLoading } = useWorkspace()
  if (organizationsLoading) {
    return <p role="status">{t('common.loading')}</p>
  }
  if (!org || !organizations.some((organization) => organization.slug === org)) {
    return <NotFoundPage />
  }
  return <>{children}</>
}
