'use client'

import { Code2, Cpu, Globe, Layers, Monitor, Music } from 'lucide-react'
import { motion } from 'motion/react'
import { Section, SectionHeader } from './section'

const features = [
  {
    icon: Cpu,
    title: 'One Daemon, Every Device',
    description:
      'A single process orchestrates all your RGB hardware — keyboards, mice, LED strips, and case lighting through a unified pipeline.',
    color: 'text-sc-purple',
    borderColor: 'hover:border-sc-purple/50',
    glow: 'hover:shadow-[0_0_30px_oklch(0.58_0.29_315/0.1)]',
  },
  {
    icon: Globe,
    title: 'Effects Are Web Pages',
    description:
      'Write effects in HTML Canvas, WebGL, or pure GLSL. An embedded Servo browser renders them and samples onto your physical LEDs at 60fps.',
    color: 'text-sc-cyan',
    borderColor: 'hover:border-sc-cyan/50',
    glow: 'hover:shadow-[0_0_30px_oklch(0.89_0.16_178/0.1)]',
  },
  {
    icon: Music,
    title: 'Audio-Reactive',
    description:
      'Full FFT analysis with beat detection and BPM tracking. Bass, mid, treble — your lights move to your music in real time.',
    color: 'text-sc-coral',
    borderColor: 'hover:border-sc-coral/50',
    glow: 'hover:shadow-[0_0_30px_oklch(0.7_0.22_350/0.1)]',
  },
  {
    icon: Layers,
    title: 'Spatial Mapping',
    description:
      'Drag-and-drop layout editor maps your physical desk to a virtual canvas. Position devices with rotation and scaling.',
    color: 'text-sc-yellow',
    borderColor: 'hover:border-sc-yellow/50',
    glow: 'hover:shadow-[0_0_30px_oklch(0.93_0.14_110/0.08)]',
  },
  {
    icon: Monitor,
    title: 'Linux & Mac',
    description:
      'Built for Linux from day one, with USB device support on macOS. HID, SMBus, and UDP protocols all native.',
    color: 'text-sc-green',
    borderColor: 'hover:border-sc-green/50',
    glow: 'hover:shadow-[0_0_30px_oklch(0.83_0.22_155/0.1)]',
  },
  {
    icon: Code2,
    title: 'Fully Open Source',
    description:
      'No subscriptions, no telemetry, no vendor lock-in. Community-extensible with a TypeScript SDK for creating your own effects.',
    color: 'text-sc-purple',
    borderColor: 'hover:border-sc-purple/50',
    glow: 'hover:shadow-[0_0_30px_oklch(0.58_0.29_315/0.1)]',
  },
]

export function Features() {
  return (
    <Section id="features">
      <SectionHeader
        subtitle="One engine to orchestrate every RGB device on your desk. Powered by web standards, driven by Rust."
        title="Built Different"
      />

      <div className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
        {features.map((feature, i) => (
          <motion.div
            className={`group rounded-2xl border border-sc-border bg-sc-bg-base p-8 transition-all duration-300 ${feature.borderColor} ${feature.glow} hover:bg-sc-bg-highlight/50`}
            initial={{ opacity: 0, y: 20 }}
            key={feature.title}
            transition={{ delay: i * 0.08, duration: 0.5 }}
            viewport={{ once: true }}
            whileInView={{ opacity: 1, y: 0 }}
          >
            <feature.icon className={`mb-4 h-8 w-8 ${feature.color}`} />
            <h3 className="mb-3 font-heading text-lg font-semibold text-sc-fg-primary">{feature.title}</h3>
            <p className="font-body text-sm leading-relaxed text-sc-fg-muted">{feature.description}</p>
          </motion.div>
        ))}
      </div>
    </Section>
  )
}
