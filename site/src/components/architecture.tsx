'use client'

import { motion } from 'motion/react'
import { Section, SectionHeader } from './section'

const layers = [
  {
    label: 'Input Sources',
    items: ['Audio FFT', 'Screen Capture', 'System Events'],
    color: 'border-sc-purple/60',
    bg: 'bg-sc-purple/8',
  },
  {
    label: 'Effect Engine',
    items: ['Servo Browser', 'wgpu Compute', 'GLSL Shaders'],
    color: 'border-sc-cyan/60',
    bg: 'bg-sc-cyan/8',
    highlight: true,
  },
  {
    label: 'Spatial Sampler',
    items: ['Canvas to LED Mapping', '320x200 RGBA', 'Oklab Color Math'],
    color: 'border-sc-coral/60',
    bg: 'bg-sc-coral/8',
  },
  {
    label: 'Hardware Drivers',
    items: ['Razer USB', 'WLED UDP', 'Corsair HID', 'ASUS SMBus'],
    color: 'border-sc-green/60',
    bg: 'bg-sc-green/8',
  },
]

const stats = [
  { value: '60', unit: 'fps', label: 'render loop' },
  { value: '23+', unit: '', label: 'built-in effects' },
  { value: '6', unit: '', label: 'device protocols' },
  { value: '4', unit: '', label: 'render tiers' },
]

export function Architecture() {
  return (
    <Section id="architecture">
      <SectionHeader
        subtitle="From input sources through the render engine to physical LEDs — a complete pipeline built in Rust."
        title="Under the Hood"
      />

      {/* Pipeline diagram */}
      <div className="mx-auto max-w-2xl space-y-3">
        {layers.map((layer, i) => (
          <motion.div
            initial={{ opacity: 0, x: i % 2 === 0 ? -20 : 20 }}
            key={layer.label}
            transition={{ delay: i * 0.1, duration: 0.5 }}
            viewport={{ once: true }}
            whileInView={{ opacity: 1, x: 0 }}
          >
            <div className={`rounded-xl border ${layer.color} ${layer.bg} p-6 ${layer.highlight ? 'glow-cyan' : ''}`}>
              <h3 className="mb-3 font-heading text-sm font-semibold uppercase tracking-wider text-sc-fg-primary">
                {layer.label}
              </h3>
              <div className="flex flex-wrap gap-2">
                {layer.items.map((item) => (
                  <span
                    className="rounded-md border border-sc-border bg-sc-bg-dark/60 px-3 py-1.5 font-mono text-xs text-sc-fg-muted"
                    key={item}
                  >
                    {item}
                  </span>
                ))}
              </div>
            </div>

            {/* Connector */}
            {i < layers.length - 1 && (
              <div className="flex justify-center py-1">
                <div className="h-3 w-px bg-sc-fg-subtle/40" />
              </div>
            )}
          </motion.div>
        ))}
      </div>

      {/* Stats strip */}
      <motion.div
        className="mt-16 grid grid-cols-2 gap-8 sm:grid-cols-4"
        initial={{ opacity: 0 }}
        transition={{ delay: 0.4, duration: 0.6 }}
        viewport={{ once: true }}
        whileInView={{ opacity: 1 }}
      >
        {stats.map((stat) => (
          <div className="text-center" key={stat.label}>
            <div className="font-heading text-4xl font-bold text-sc-fg-primary">
              {stat.value}
              {stat.unit && <span className="text-sc-cyan">{stat.unit}</span>}
            </div>
            <p className="mt-1 font-body text-sm text-sc-fg-muted">{stat.label}</p>
          </div>
        ))}
      </motion.div>
    </Section>
  )
}
