import type { HTMLAttributes, ReactNode, CSSProperties } from 'react'

export function Panel({
  children,
  style,
  ...rest
}: HTMLAttributes<HTMLDivElement> & {
  children: ReactNode
  style?: CSSProperties
}) {
  return (
    <div
      style={{
        background: '#fff',
        border: '1px solid #d8dee4',
        borderRadius: '0.5rem',
        padding: '1rem',
        ...style,
      }}
      {...rest}
    >
      {children}
    </div>
  )
}
