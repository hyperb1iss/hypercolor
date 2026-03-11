import { Exo_2, Jura, Space_Mono } from 'next/font/google'

export const jura = Jura({
  display: 'swap',
  subsets: ['latin'],
  variable: '--font-jura',
  weight: ['300', '400', '500', '600', '700'],
})

export const exo2 = Exo_2({
  display: 'swap',
  subsets: ['latin'],
  variable: '--font-exo2',
  weight: ['300', '400', '500', '600', '700'],
})

export const spaceMono = Space_Mono({
  display: 'swap',
  subsets: ['latin'],
  variable: '--font-space-mono',
  weight: ['400', '700'],
})
