'use client'

import { motion } from 'motion/react'
import type { ReactNode } from 'react'

interface SectionProps {
  id: string
  children: ReactNode
  className?: string
}

export function Section({ id, children, className = '' }: SectionProps) {
  return (
    <section className={`relative py-24 sm:py-32 ${className}`} id={id}>
      <motion.div
        className="mx-auto max-w-7xl px-6"
        initial={{ opacity: 0, y: 40 }}
        transition={{ duration: 0.6, ease: 'easeOut' }}
        viewport={{ once: true, margin: '-100px' }}
        whileInView={{ opacity: 1, y: 0 }}
      >
        {children}
      </motion.div>
    </section>
  )
}

interface SectionHeaderProps {
  title: string
  subtitle?: string
  gradient?: boolean
}

export function SectionHeader({ title, subtitle, gradient = false }: SectionHeaderProps) {
  return (
    <div className="mb-16 text-center">
      <h2
        className={`font-heading text-3xl font-bold tracking-tight sm:text-4xl lg:text-5xl ${gradient ? 'text-gradient-hero' : 'text-sc-fg-primary'}`}
      >
        {title}
      </h2>
      {subtitle && <p className="mx-auto mt-4 max-w-2xl font-body text-lg text-sc-fg-muted">{subtitle}</p>}
    </div>
  )
}
