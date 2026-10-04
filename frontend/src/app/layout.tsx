/**
 * Root layout: the one Server Component in the app. It owns the <html> shell,
 * the <title>, and the global stylesheet (Next only allows global CSS to be
 * imported here). Everything interactive lives below it in client components.
 */
import type { Metadata } from 'next'
import type { ReactNode } from 'react'
import { Nav } from '@/Nav'
import './globals.css'

export const metadata: Metadata = {
  title: 'Morning Report',
}

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>
        <Nav />
        {children}
      </body>
    </html>
  )
}
