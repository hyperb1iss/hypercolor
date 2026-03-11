'use client'

import { motion } from 'motion/react'
import { Section, SectionHeader } from './section'
import { ShaderCanvas } from './shader-canvas'
import { PLASMA_SHADER } from './shaders'

const codeExample = `import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Borealis', shader, {
    speed:          [1, 10, 5],
    intensity:      [0, 100, 82],
    curtainHeight:  [20, 90, 55],
    palette:        ['Northern Lights', 'SilkCircuit', 'Cyberpunk'],
}, {
    description: 'Aurora borealis ribbons with starfield',
})`

const tiers = [
  { name: 'GLSL', desc: 'Single .glsl file', color: 'bg-sc-purple' },
  { name: 'TS + Shader', desc: '11-line binding', color: 'bg-sc-cyan' },
  { name: 'Canvas 2D', desc: 'Stateful draw loop', color: 'bg-sc-coral' },
  { name: 'Full OOP', desc: 'Class-based lifecycle', color: 'bg-sc-green' },
]

export function SDKPreview() {
  return (
    <Section id="sdk">
      <SectionHeader
        subtitle="Effects are literally web pages. We support the LightScript API plus a full TypeScript and GLSL SDK for building your own."
        title="Developer SDK"
      />

      <div className="grid items-start gap-8 lg:grid-cols-2">
        {/* Code panel */}
        <motion.div
          className="overflow-hidden rounded-2xl border border-sc-border bg-sc-bg-base"
          initial={{ opacity: 0, x: -20 }}
          transition={{ duration: 0.5 }}
          viewport={{ once: true }}
          whileInView={{ opacity: 1, x: 0 }}
        >
          {/* Editor chrome */}
          <div className="flex items-center gap-2 border-b border-sc-border px-4 py-3">
            <div className="h-3 w-3 rounded-full bg-sc-red/60" />
            <div className="h-3 w-3 rounded-full bg-sc-yellow/60" />
            <div className="h-3 w-3 rounded-full bg-sc-green/60" />
            <span className="ml-4 font-mono text-xs text-sc-fg-subtle">borealis/main.ts</span>
          </div>

          <pre className="overflow-x-auto p-6">
            <code className="font-mono text-sm leading-relaxed text-sc-fg-muted">{codeExample}</code>
          </pre>

          {/* Tier badges */}
          <div className="border-t border-sc-border px-6 py-4">
            <p className="mb-3 font-mono text-xs text-sc-fg-subtle">4 progressive tiers:</p>
            <div className="flex flex-wrap gap-2">
              {tiers.map((tier) => (
                <div className="flex items-center gap-2" key={tier.name}>
                  <div className={`h-2 w-2 rounded-full ${tier.color}`} />
                  <span className="font-mono text-xs text-sc-fg-muted">
                    <span className="text-sc-fg-primary">{tier.name}</span> — {tier.desc}
                  </span>
                </div>
              ))}
            </div>
          </div>
        </motion.div>

        {/* Live demo panel */}
        <motion.div
          className="overflow-hidden rounded-2xl border border-sc-border bg-sc-bg-base"
          initial={{ opacity: 0, x: 20 }}
          transition={{ duration: 0.5 }}
          viewport={{ once: true }}
          whileInView={{ opacity: 1, x: 0 }}
        >
          <div className="flex items-center justify-between border-b border-sc-border px-4 py-3">
            <span className="font-mono text-xs text-sc-fg-subtle">Live Preview — Plasma Engine</span>
            <span className="inline-flex items-center gap-1.5 rounded-full bg-sc-green/10 px-2 py-0.5">
              <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-sc-green" />
              <span className="font-mono text-[10px] text-sc-green">60fps</span>
            </span>
          </div>

          <div className="aspect-video">
            <ShaderCanvas
              fragmentShader={PLASMA_SHADER}
              uniforms={{
                iBackgroundColor: [0.02, 0.01, 0.06],
                iColor1: [0.88, 0.14, 1.0],
                iColor2: [0.5, 1.0, 0.92],
                iColor3: [0.16, 0.48, 1.0],
                iTheme: 0,
                iSpeed: 5,
                iBloom: 65,
                iSpread: 50,
                iDensity: 55,
              }}
            />
          </div>

          <div className="border-t border-sc-border px-6 py-4">
            <p className="font-body text-sm text-sc-fg-muted">
              This plasma effect is running <span className="text-sc-cyan">live in your browser</span> using the same
              GLSL shader that drives real LED hardware.
            </p>
          </div>
        </motion.div>
      </div>
    </Section>
  )
}
