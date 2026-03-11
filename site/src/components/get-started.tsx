'use client'

import { Check, Copy } from 'lucide-react'
import { motion } from 'motion/react'
import { useCallback, useState } from 'react'
import { Section, SectionHeader } from './section'

const installMethods = [
  {
    id: 'cargo',
    label: 'Cargo',
    command: 'cargo install hypercolor',
  },
  {
    id: 'aur',
    label: 'AUR',
    command: 'paru -S hypercolor',
  },
  {
    id: 'source',
    label: 'Source',
    command: 'git clone https://github.com/hyperb1iss/hypercolor && cd hypercolor && cargo build --release',
  },
]

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)

  const copy = useCallback(() => {
    navigator.clipboard.writeText(text)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }, [text])

  return (
    <button
      aria-label="Copy to clipboard"
      className="rounded-md p-1.5 text-sc-fg-subtle transition-colors hover:bg-sc-bg-highlight hover:text-sc-fg-muted"
      onClick={copy}
      type="button"
    >
      {copied ? <Check className="text-sc-green" size={14} /> : <Copy size={14} />}
    </button>
  )
}

export function GetStarted() {
  const [active, setActive] = useState(0)

  return (
    <Section id="get-started">
      <SectionHeader
        gradient={true}
        subtitle="Get up and running in minutes. Choose your preferred installation method."
        title="Get Started"
      />

      <div className="mx-auto max-w-2xl">
        {/* Method tabs */}
        <div className="mb-4 flex gap-2">
          {installMethods.map((method, i) => (
            <button
              className={`rounded-lg px-4 py-2 font-mono text-sm transition-all ${
                i === active
                  ? 'bg-sc-purple/20 text-sc-purple'
                  : 'text-sc-fg-subtle hover:bg-sc-bg-highlight hover:text-sc-fg-muted'
              }`}
              key={method.id}
              onClick={() => setActive(i)}
              type="button"
            >
              {method.label}
            </button>
          ))}
        </div>

        {/* Command display */}
        <motion.div
          animate={{ opacity: 1 }}
          className="flex items-center justify-between rounded-xl border border-sc-border bg-sc-bg-base p-5"
          initial={{ opacity: 0.5 }}
          key={active}
          transition={{ duration: 0.2 }}
        >
          <code className="overflow-x-auto font-mono text-sm text-sc-fg-muted">
            <span className="mr-2 text-sc-cyan">$</span>
            {installMethods[active].command}
          </code>
          <CopyButton text={installMethods[active].command} />
        </motion.div>

        {/* Quick start */}
        <div className="mt-8 rounded-xl border border-sc-border bg-sc-bg-base/50 p-6">
          <h3 className="mb-4 font-heading text-sm font-semibold uppercase tracking-wider text-sc-fg-primary">
            Then run it
          </h3>
          <div className="space-y-3">
            {[
              { cmd: 'hyper daemon', desc: 'Start the daemon' },
              { cmd: 'hyper effects list', desc: 'Browse effects' },
              { cmd: 'hyper apply borealis', desc: 'Apply an effect' },
            ].map((step) => (
              <div className="flex items-center justify-between" key={step.cmd}>
                <div className="flex items-center gap-3">
                  <code className="font-mono text-sm text-sc-cyan">{step.cmd}</code>
                  <span className="font-body text-xs text-sc-fg-subtle">{step.desc}</span>
                </div>
                <CopyButton text={step.cmd} />
              </div>
            ))}
          </div>
        </div>
      </div>
    </Section>
  )
}
