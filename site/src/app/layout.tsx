import type { Metadata } from 'next'
import { exo2, jura, spaceMono } from './fonts'
import './globals.css'

export const metadata: Metadata = {
  title: 'Hypercolor — RGB Lighting Orchestration for Linux',
  description:
    'Open-source RGB lighting engine. Effects are web pages. Your desk is the canvas. One daemon for every device, powered by Rust + Servo.',
  keywords: ['RGB', 'lighting', 'Linux', 'Rust', 'open-source', 'LED', 'effects', 'keyboard', 'gaming'],
  authors: [{ name: 'hyperbliss', url: 'https://github.com/hyperb1iss' }],
  openGraph: {
    title: 'Hypercolor — RGB Lighting Orchestration for Linux',
    description: 'Effects are web pages. Your desk is the canvas.',
    type: 'website',
    siteName: 'Hypercolor',
  },
  twitter: {
    card: 'summary_large_image',
    title: 'Hypercolor — RGB Lighting Orchestration for Linux',
    description: 'Effects are web pages. Your desk is the canvas.',
  },
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html className={`${jura.variable} ${exo2.variable} ${spaceMono.variable}`} lang="en">
      <body className="antialiased">{children}</body>
    </html>
  )
}
