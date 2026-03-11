'use client'

import { ArrowRight, Github } from 'lucide-react'
import { motion } from 'motion/react'
import { ShaderCanvas } from './shader-canvas'
import { NEBULA_SHADER } from './shaders'

export function Hero() {
  return (
    <section className="relative flex min-h-screen items-center justify-center overflow-hidden">
      {/* Live shader background */}
      <div className="absolute inset-0">
        <ShaderCanvas
          fragmentShader={NEBULA_SHADER}
          speed={0.4}
          uniforms={{
            iSpeed: 3,
            iCloudDensity: 65,
            iWarpStrength: 50,
            iStarField: 80,
            iSaturation: 95,
            iContrast: 95,
            iPalette: 0,
          }}
        />
        {/* Gradient overlay for text readability */}
        <div className="absolute inset-0 bg-gradient-to-b from-sc-bg-dark/60 via-sc-bg-dark/20 to-sc-bg-dark" />
      </div>

      <div className="relative z-10 mx-auto max-w-5xl px-6 text-center">
        <motion.div
          animate={{ opacity: 1, y: 0 }}
          initial={{ opacity: 0, y: 30 }}
          transition={{ duration: 0.8, ease: 'easeOut' }}
        >
          {/* Title */}
          <h1 className="mb-6 font-heading text-5xl leading-tight font-bold tracking-tight sm:text-7xl lg:text-8xl">
            <span className="text-gradient-hero">hyper</span>
            <span className="text-sc-fg-primary">color</span>
          </h1>

          {/* Tagline */}
          <p className="mx-auto mb-4 max-w-2xl font-body text-xl text-sc-fg-primary/80 sm:text-2xl">
            Effects are web pages.
            <br />
            <span className="text-sc-fg-primary">Your desk is the canvas.</span>
          </p>

          {/* Sub-tagline */}
          <p className="mx-auto mb-10 max-w-xl font-body text-base text-sc-fg-muted">
            RGB lighting orchestration for Linux and macOS. One daemon for every device — keyboards, mice, LED strips,
            case lighting — driven by HTML Canvas, WebGL, and GLSL shaders at 60fps.
          </p>

          {/* CTAs */}
          <div className="flex flex-col items-center justify-center gap-4 sm:flex-row">
            <a
              className="group inline-flex items-center gap-2 rounded-xl bg-gradient-to-r from-sc-purple to-sc-cyan px-8 py-3.5 font-heading text-sm font-semibold text-white shadow-lg transition-all hover:shadow-sc-purple/25 hover:shadow-2xl"
              href="#get-started"
            >
              Get Started
              <ArrowRight className="transition-transform group-hover:translate-x-0.5" size={16} />
            </a>
            <a
              className="inline-flex items-center gap-2 rounded-xl border border-sc-border bg-sc-bg-base/50 px-8 py-3.5 font-heading text-sm font-semibold text-sc-fg-primary backdrop-blur-sm transition-all hover:border-sc-border-highlight hover:bg-sc-bg-highlight/50"
              href="https://github.com/hyperb1iss/hypercolor"
              rel="noopener noreferrer"
              target="_blank"
            >
              <Github size={16} />
              View on GitHub
            </a>
          </div>
        </motion.div>

        {/* Tech badges */}
        <motion.div
          animate={{ opacity: 1 }}
          className="mt-16 flex flex-wrap items-center justify-center gap-3"
          initial={{ opacity: 0 }}
          transition={{ delay: 0.5, duration: 0.6 }}
        >
          {['Rust', 'Servo', 'WebGL', 'Audio FFT', '60fps', 'Open Source'].map((label) => (
            <span
              className="rounded-full border border-sc-purple/30 bg-sc-bg-base/50 px-4 py-1.5 font-mono text-xs text-sc-fg-muted shadow-[0_0_12px_oklch(0.58_0.29_315/0.15)] backdrop-blur-sm transition-all duration-300 hover:border-sc-purple/60 hover:text-sc-fg-primary hover:shadow-[0_0_20px_oklch(0.58_0.29_315/0.3)]"
              key={label}
            >
              {label}
            </span>
          ))}
        </motion.div>
      </div>
    </section>
  )
}
