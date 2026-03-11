'use client'

import { motion } from 'motion/react'
import Image from 'next/image'
import { useState } from 'react'
import { Section, SectionHeader } from './section'

const tabs = [
  {
    id: 'dashboard',
    label: 'Dashboard',
    image: '/images/dashboard.png',
    description: 'Live effect preview with real-time controls, device status, and audio visualization.',
  },
  {
    id: 'effects',
    label: 'Effects Browser',
    image: '/images/effects-browser.png',
    description:
      'Browse, search, and filter from 23+ built-in effects. Favorites, categories, and audio-reactive filters.',
  },
  {
    id: 'controls',
    label: 'Effect Controls',
    image: '/images/effect-controls.png',
    description: 'Auto-generated controls — sliders, dropdowns, color pickers — derived from effect metadata.',
  },
  {
    id: 'layout',
    label: 'Layout Editor',
    image: '/images/layout-editor.png',
    description:
      'Drag-and-drop spatial mapping. Position your devices on a 2D canvas to create unified lighting scenes.',
  },
  {
    id: 'devices',
    label: 'Devices',
    image: '/images/devices.png',
    description: 'Manage connected hardware. Discover, identify, and configure device zones and LED mappings.',
  },
]

export function Showcase() {
  const [activeTab, setActiveTab] = useState(0)

  return (
    <Section className="overflow-hidden" id="showcase">
      <SectionHeader
        gradient={true}
        subtitle="A beautiful web UI that feels native. Dark surfaces, vivid previews — the light is the hero."
        title="See It In Action"
      />

      {/* Tab bar */}
      <div className="mb-8 flex flex-wrap justify-center gap-2">
        {tabs.map((tab, i) => (
          <button
            className={`rounded-lg px-4 py-2 font-mono text-sm transition-all ${
              i === activeTab
                ? 'bg-sc-purple/20 text-sc-purple'
                : 'text-sc-fg-subtle hover:bg-sc-bg-highlight hover:text-sc-fg-muted'
            }`}
            key={tab.id}
            onClick={() => setActiveTab(i)}
            type="button"
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Screenshot display */}
      <motion.div
        animate={{ opacity: 1, scale: 1 }}
        className="relative mx-auto max-w-5xl overflow-hidden rounded-2xl border border-sc-border bg-sc-bg-base shadow-2xl"
        initial={{ opacity: 0.5, scale: 0.98 }}
        key={activeTab}
        transition={{ duration: 0.3 }}
      >
        {/* Window chrome */}
        <div className="flex items-center gap-2 border-b border-sc-border px-4 py-3">
          <div className="h-3 w-3 rounded-full bg-sc-red/60" />
          <div className="h-3 w-3 rounded-full bg-sc-yellow/60" />
          <div className="h-3 w-3 rounded-full bg-sc-green/60" />
          <span className="ml-4 font-mono text-xs text-sc-fg-subtle">hypercolor — {tabs[activeTab].label}</span>
        </div>

        <div className="relative aspect-video bg-sc-bg-dark">
          <Image
            alt={tabs[activeTab].label}
            className="object-cover"
            fill={true}
            priority={activeTab === 0}
            sizes="(max-width: 1024px) 100vw, 1024px"
            src={tabs[activeTab].image}
          />
        </div>
      </motion.div>

      {/* Description */}
      <p className="mt-6 text-center font-body text-sc-fg-muted">{tabs[activeTab].description}</p>
    </Section>
  )
}
