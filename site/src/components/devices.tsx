'use client'

import { motion } from 'motion/react'
import Image from 'next/image'
import { Section, SectionHeader } from './section'

/* ── Dygma inline SVG (no public logo available) ── */
function DygmaLogo() {
  return (
    <svg aria-label="Dygma" fill="currentColor" role="img" viewBox="0 0 32 32">
      <rect height="12" rx="2" width="12" x="1" y="10" />
      <rect height="8" rx="1.5" width="8" x="2.5" y="12" />
      <rect height="12" rx="2" width="12" x="19" y="10" />
      <rect height="8" rx="1.5" width="8" x="21.5" y="12" />
    </svg>
  )
}

/* ── Brand data (sorted alphabetically) ── */

const deviceBrands = [
  {
    name: 'Ableton',
    protocol: 'USB HID',
    devices: 'Push 2 display and RGB pads',
    logo: '/logos/ableton.svg',
    invert: true,
    accent: 'border-sc-cyan/40',
    glow: 'hover:shadow-[0_0_24px_oklch(0.89_0.16_178/0.15)]',
    strip: 'from-sc-cyan/60 to-sc-cyan/0',
  },
  {
    name: 'ASUS',
    protocol: 'SMBus / I2C',
    devices: 'Motherboards, ROG, AURA-compatible',
    logo: '/logos/asus-rog.svg',
    invert: true,
    accent: 'border-sc-red/40',
    glow: 'hover:shadow-[0_0_24px_oklch(0.65_0.22_25/0.15)]',
    strip: 'from-sc-red/60 to-sc-red/0',
  },
  {
    name: 'Corsair',
    protocol: 'USB HID',
    devices: 'Lighting Node, Link, Keyboards, Mice',
    logo: '/logos/corsair.svg',
    invert: true,
    accent: 'border-sc-yellow/40',
    glow: 'hover:shadow-[0_0_24px_oklch(0.93_0.14_110/0.15)]',
    strip: 'from-sc-yellow/60 to-sc-yellow/0',
  },
  {
    name: 'Dygma',
    protocol: 'Firmware Stream',
    devices: 'Defy keyboard with per-key RGB',
    logo: null,
    invert: false,
    accent: 'border-sc-purple/40',
    glow: 'hover:shadow-[0_0_24px_oklch(0.58_0.29_315/0.15)]',
    strip: 'from-sc-purple/60 to-sc-purple/0',
  },
  {
    name: 'PrismRGB',
    protocol: 'USB HID',
    devices: 'Prism 8, Prism S, Mini, Nollie 8',
    logo: '/logos/prismrgb.png',
    invert: false,
    accent: 'border-sc-coral/40',
    glow: 'hover:shadow-[0_0_24px_oklch(0.7_0.22_350/0.15)]',
    strip: 'from-sc-coral/60 to-sc-coral/0',
  },
  {
    name: 'QMK',
    protocol: 'USB HID',
    devices: 'Custom keyboards with per-key RGB',
    logo: '/logos/qmk.svg',
    invert: false,
    accent: 'border-sc-yellow/40',
    glow: 'hover:shadow-[0_0_24px_oklch(0.93_0.14_110/0.15)]',
    strip: 'from-sc-yellow/60 to-sc-yellow/0',
  },
  {
    name: 'Razer',
    protocol: 'USB HID',
    devices: 'Huntsman V2, Basilisk V3, Blade, Seiren',
    logo: '/logos/razer.svg',
    invert: false,
    accent: 'border-sc-green/40',
    glow: 'hover:shadow-[0_0_24px_oklch(0.83_0.22_155/0.15)]',
    strip: 'from-sc-green/60 to-sc-green/0',
  },
  {
    name: 'WLED',
    protocol: 'UDP / DDP',
    devices: 'LED strips, matrices, any WLED device',
    logo: '/logos/wled.svg',
    invert: false,
    accent: 'border-sc-cyan/40',
    glow: 'hover:shadow-[0_0_24px_oklch(0.89_0.16_178/0.15)]',
    strip: 'from-sc-cyan/60 to-sc-cyan/0',
  },
]

const protocols = [
  { name: 'USB HID', desc: 'Direct hardware control on Linux and macOS' },
  { name: 'UDP / DDP', desc: 'Networked LED strips via mDNS discovery' },
  { name: 'MIDI', desc: 'Pad and key lighting for controllers' },
  { name: 'SMBus / I2C', desc: 'Motherboard RGB via kernel driver' },
  { name: 'Serial', desc: 'Legacy device support' },
]

export function Devices() {
  return (
    <Section id="devices">
      <SectionHeader
        subtitle="Native drivers for real hardware on Linux and macOS. Every protocol decoded, every device speaking one language."
        title="Hardware Support"
      />

      {/* Brand grid */}
      <div className="mb-16 grid gap-5 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
        {deviceBrands.map((brand, i) => (
          <motion.div
            className={`group relative overflow-hidden rounded-xl border bg-sc-bg-base transition-all duration-300 ${brand.accent} ${brand.glow} hover:bg-sc-bg-highlight/40`}
            initial={{ opacity: 0, y: 16 }}
            key={brand.name}
            transition={{ delay: i * 0.06, duration: 0.4 }}
            viewport={{ once: true }}
            whileInView={{ opacity: 1, y: 0 }}
          >
            {/* Colored accent strip at top */}
            <div className={`h-0.5 bg-gradient-to-r ${brand.strip}`} />

            <div className="p-6">
              {/* Logo + protocol row */}
              <div className="mb-4 flex items-center justify-between">
                <div className="flex h-10 w-10 items-center justify-center">
                  {brand.logo ? (
                    <Image
                      alt={`${brand.name} logo`}
                      className={`h-8 w-auto max-w-[40px] object-contain ${brand.invert ? 'invert' : ''}`}
                      height={40}
                      src={brand.logo}
                      width={40}
                    />
                  ) : (
                    <div className="h-8 w-8 text-sc-fg-muted">
                      <DygmaLogo />
                    </div>
                  )}
                </div>
                <span className="rounded-md border border-sc-border bg-sc-bg-dark px-2 py-0.5 font-mono text-[10px] text-sc-fg-subtle">
                  {brand.protocol}
                </span>
              </div>

              <h3 className="mb-1.5 font-heading text-base font-semibold text-sc-fg-primary">{brand.name}</h3>
              <p className="font-body text-sm text-sc-fg-muted">{brand.devices}</p>
            </div>
          </motion.div>
        ))}
      </div>

      {/* Protocol legend */}
      <div className="mx-auto max-w-3xl rounded-2xl border border-sc-border bg-sc-bg-base p-8">
        <h3 className="mb-6 text-center font-heading text-lg font-semibold text-sc-fg-primary">Transport Layer</h3>
        <div className="grid gap-4 sm:grid-cols-2">
          {protocols.map((proto) => (
            <div className="flex items-start gap-3" key={proto.name}>
              <div className="mt-1.5 h-2 w-2 shrink-0 rounded-full bg-sc-cyan" />
              <div>
                <span className="font-mono text-sm text-sc-fg-primary">{proto.name}</span>
                <p className="font-body text-xs text-sc-fg-subtle">{proto.desc}</p>
              </div>
            </div>
          ))}
        </div>
      </div>
    </Section>
  )
}
