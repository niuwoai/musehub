import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import App from './App'
import './i18n'

describe('App', () => {
  it('默认以中文显示产品名与版本', () => {
    render(<App />)

    expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('MuseHub')
    expect(screen.getByText(/未发布的开发版/)).toBeTruthy()
    expect(screen.getByText(/0\.1\.0-rc4/)).toBeTruthy()
    expect(screen.getByRole('button', { name: /登录/ })).toBeTruthy()
  })
})
