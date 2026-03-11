'use client'

import { Github, Menu, X } from 'lucide-react'
import { AnimatePresence, motion } from 'motion/react'
import { useEffect, useState } from 'react'

const sections = [
  { id: 'features', label: 'Features' },
  { id: 'showcase', label: 'Showcase' },
  { id: 'sdk', label: 'SDK' },
  { id: 'devices', label: 'Devices' },
  { id: 'get-started', label: 'Get Started' },
]

export function Nav() {
  const [scrolled, setScrolled] = useState(false)
  const [mobileOpen, setMobileOpen] = useState(false)

  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 40)
    window.addEventListener('scroll', onScroll, { passive: true })
    return () => window.removeEventListener('scroll', onScroll)
  }, [])

  return (
    <nav
      className={`fixed top-0 z-50 w-full transition-all duration-300 ${
        scrolled ? 'glass border-b border-sc-border shadow-lg' : 'bg-transparent'
      }`}
    >
      <div className="mx-auto flex max-w-7xl items-center justify-between px-6 py-4">
        <span className="font-heading text-xl font-bold tracking-wider text-sc-fg-primary">
          <span className="text-gradient-hero">hyper</span>color
        </span>

        {/* Desktop links */}
        <div className="hidden items-center gap-8 md:flex">
          {sections.map((s) => (
            <a
              className="font-body text-sm text-sc-fg-muted transition-colors hover:text-sc-cyan"
              href={`#${s.id}`}
              key={s.id}
            >
              {s.label}
            </a>
          ))}
          <a
            className="flex items-center gap-2 rounded-lg border border-sc-border px-4 py-2 font-mono text-sm text-sc-fg-muted transition-all hover:border-sc-purple/50 hover:text-sc-fg-primary hover:shadow-[0_0_16px_oklch(0.58_0.29_315/0.12)]"
            href="https://github.com/hyperb1iss/hypercolor"
            rel="noopener noreferrer"
            target="_blank"
          >
            <Github size={16} />
            GitHub
          </a>
        </div>

        {/* Mobile menu button */}
        <button
          aria-label={mobileOpen ? 'Close menu' : 'Open menu'}
          className="text-sc-fg-muted md:hidden"
          onClick={() => setMobileOpen(!mobileOpen)}
          type="button"
        >
          {mobileOpen ? <X size={24} /> : <Menu size={24} />}
        </button>
      </div>

      {/* Mobile menu */}
      <AnimatePresence>
        {mobileOpen && (
          <motion.div
            animate={{ opacity: 1, y: 0 }}
            className="glass border-b border-sc-border px-6 pb-6 md:hidden"
            exit={{ opacity: 0, y: -10 }}
            initial={{ opacity: 0, y: -10 }}
          >
            {sections.map((s) => (
              <a
                className="block py-3 font-body text-sc-fg-muted transition-colors hover:text-sc-cyan"
                href={`#${s.id}`}
                key={s.id}
                onClick={() => setMobileOpen(false)}
              >
                {s.label}
              </a>
            ))}
          </motion.div>
        )}
      </AnimatePresence>
    </nav>
  )
}
