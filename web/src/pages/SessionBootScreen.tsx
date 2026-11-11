import { useTranslation } from 'react-i18next'

export function SessionBootScreen() {
  const { t } = useTranslation()
  return (
    <main className="app-shell" data-testid="session-boot">
      <p role="status">{t('common.loading')}</p>
    </main>
  )
}
