import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import './i18n'

const container = document.getElementById('root')

if (!container) {
  throw new Error('Missing #root container')
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
