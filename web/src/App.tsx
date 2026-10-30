import { useTranslation } from 'react-i18next'

const appVersion = import.meta.env.VITE_APP_VERSION

export default function App() {
  const { t, i18n } = useTranslation()
  const isChinese = i18n.language.startsWith('zh')
  const nextLanguage = isChinese ? 'en' : 'zh'

  return (
    <main
      style={{
        minHeight: '100vh',
        margin: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: '#f6f7f9',
        fontFamily:
          '-apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans SC", Roboto, sans-serif',
        color: '#1f2328',
      }}
    >
      <section style={{ width: 'min(32rem, 90vw)', padding: '2rem' }}>
        <h1 style={{ margin: '0 0 0.75rem', fontSize: '1.75rem' }}>{t('app.name')}</h1>
        <p style={{ margin: '0 0 1.5rem', lineHeight: 1.6, color: '#3a4048' }}>
          {t('app.tagline', { version: appVersion })}
        </p>
        <div
          style={{
            display: 'flex',
            flexWrap: 'wrap',
            alignItems: 'center',
            gap: '1rem',
            borderTop: '1px solid #d8dee4',
            paddingTop: '1rem',
          }}
        >
          <span style={{ fontSize: '0.875rem', color: '#57606a' }}>
            {t('app.versionLabel')} <code>{appVersion}</code>
          </span>
          <button
            type="button"
            onClick={() => void i18n.changeLanguage(nextLanguage)}
            style={{
              marginLeft: 'auto',
              padding: '0.4rem 0.8rem',
              borderRadius: '0.375rem',
              border: '1px solid #d0d7de',
              background: '#ffffff',
              color: 'inherit',
              font: 'inherit',
              fontSize: '0.875rem',
              cursor: 'pointer',
            }}
          >
            {t('app.switchLanguage')}
          </button>
        </div>
      </section>
    </main>
  )
}
