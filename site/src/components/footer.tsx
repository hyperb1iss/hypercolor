import { Github, Heart } from 'lucide-react'

export function Footer() {
  return (
    <footer className="border-t border-sc-border bg-sc-bg-dark py-12">
      <div className="mx-auto max-w-7xl px-6">
        <div className="flex flex-col items-center justify-between gap-6 sm:flex-row">
          {/* Brand */}
          <div>
            <span className="font-heading text-lg font-bold tracking-wider text-sc-fg-primary">
              <span className="text-gradient-hero">hyper</span>color
            </span>
            <p className="mt-1 font-body text-xs text-sc-fg-subtle">
              Open-source RGB orchestration for Linux and macOS
            </p>
          </div>

          {/* Links */}
          <div className="flex items-center gap-6">
            <a
              className="font-mono text-xs text-sc-fg-subtle transition-colors hover:text-sc-cyan"
              href="https://github.com/hyperb1iss/hypercolor"
              rel="noopener noreferrer"
              target="_blank"
            >
              <Github className="inline-block mr-1" size={14} />
              Source
            </a>
            <a
              className="font-mono text-xs text-sc-fg-subtle transition-colors hover:text-sc-cyan"
              href="https://github.com/hyperb1iss/hypercolor/blob/main/LICENSE"
              rel="noopener noreferrer"
              target="_blank"
            >
              Apache-2.0
            </a>
            <a
              className="font-mono text-xs text-sc-fg-subtle transition-colors hover:text-sc-cyan"
              href="https://github.com/hyperb1iss/hypercolor/issues"
              rel="noopener noreferrer"
              target="_blank"
            >
              Issues
            </a>
          </div>

          {/* Made with */}
          <p className="flex items-center gap-1 font-body text-xs text-sc-fg-subtle">
            Made with <Heart className="text-sc-coral" size={12} /> and Rust
          </p>
        </div>
      </div>
    </footer>
  )
}
