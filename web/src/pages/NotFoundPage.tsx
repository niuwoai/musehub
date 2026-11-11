import { useTranslation } from 'react-i18next'

export function NotFoundPage() {
  const { t } = useTranslation()
  return (
    <section data-testid="not-found">
      <h2>{t('common.notFoundTitle')}</h2>
      <p>{t('common.notFoundBody')}</p>
    </section>
  )
}
