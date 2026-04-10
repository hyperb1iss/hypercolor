// ── Hypercolor UI — Tailwind v4 config ────────────────────────────────────
//
// Token wiring lives in `tokens/primitives.css` via Tailwind v4's `@theme`
// directive. Both `tokens/primitives.css` and `tokens/semantic.css` are
// imported into `input.css`, which the Trunk pre-build hook feeds to the
// Tailwind v4 CLI. Color primitives, typography, spacing, radii, and motion
// tokens all flow through that CSS-first pipeline — Tailwind auto-generates
// utility classes from every `--color-*`, `--font-*`, `--spacing-*`,
// `--radius-*`, and `--ease-*` custom property declared under `@theme`.
//
// This JS config is kept as a compatibility surface and as quick reference
// documentation for the SilkCircuit palette. Tailwind v4 ignores it unless
// a CSS file explicitly opts in with `@config "./tailwind.config.js"`, so
// edits here have no runtime effect today. Treat `tokens/primitives.css`
// as the source of truth; mirror additions here for clarity.
//
// Available color utility classes (sampling):
//
//   Surfaces:  bg-surface-{base,raised,overlay,sunken,hover,active}
//   Text:      text-{fg-primary,fg-secondary,fg-tertiary}
//   Borders:   border-{edge-subtle,edge-default,edge-strong,edge-focus}
//   Accent:    {bg,text,border,ring}-{accent,accent-hover,accent-muted,accent-subtle}
//   Status:    {bg,text,border}-{status-success,status-error,status-warning,status-info}
//   Palette:   bg-{purple,cyan,coral,yellow,green,red,blue}
//              bg-{electric-purple,neon-cyan,electric-yellow,success-green,error-red}
//              bg-{info-blue,pink-soft}
//
// See `tokens/primitives.css` for the OKLCH source values and
// `tokens/semantic.css` for per-theme overrides (dark + light).
//
// ──────────────────────────────────────────────────────────────────────────

/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.rs",
  ],
  theme: {
    extend: {
      colors: {
        // SilkCircuit accent palette (OKLCH primitives from tokens/primitives.css)
        purple: "var(--color-purple)",
        "purple-hover": "var(--color-purple-hover)",
        "purple-light": "var(--color-purple-light)",
        cyan: "var(--color-cyan)",
        coral: "var(--color-coral)",
        yellow: "var(--color-yellow)",
        green: "var(--color-green)",
        red: "var(--color-red)",
        blue: "var(--color-blue)",

        // Legacy hex aliases (migration era — prefer OKLCH primitives above)
        "electric-purple": "var(--color-electric-purple)",
        "neon-cyan": "var(--color-neon-cyan)",
        "electric-yellow": "var(--color-electric-yellow)",
        "success-green": "var(--color-success-green)",
        "error-red": "var(--color-error-red)",
        "info-blue": "var(--color-info-blue)",
        "pink-soft": "var(--color-pink-soft)",

        // Semantic surfaces (theme-swappable via tokens/semantic.css)
        "surface-base": "var(--surface-base)",
        "surface-raised": "var(--surface-raised)",
        "surface-overlay": "var(--surface-overlay)",
        "surface-sunken": "var(--surface-sunken)",
        "surface-hover": "var(--surface-hover)",
        "surface-active": "var(--surface-active)",

        // Semantic text
        "fg-primary": "var(--text-primary)",
        "fg-secondary": "var(--text-secondary)",
        "fg-tertiary": "var(--text-tertiary)",

        // Semantic borders
        "edge-subtle": "var(--border-subtle)",
        "edge-default": "var(--border-default)",
        "edge-strong": "var(--border-strong)",
        "edge-focus": "var(--border-focus)",

        // Semantic accent
        accent: "var(--accent)",
        "accent-hover": "var(--accent-hover)",
        "accent-muted": "var(--accent-muted)",
        "accent-subtle": "var(--accent-subtle)",

        // Semantic status
        "status-success": "var(--status-success)",
        "status-error": "var(--status-error)",
        "status-warning": "var(--status-warning)",
        "status-info": "var(--status-info)",
      },
      fontFamily: {
        sans: "var(--font-sans)",
        mono: "var(--font-mono)",
        display: "var(--font-display)",
      },
      borderRadius: {
        sm: "var(--radius-sm)",
        md: "var(--radius-md)",
        lg: "var(--radius-lg)",
        xl: "var(--radius-xl)",
      },
      transitionTimingFunction: {
        silk: "var(--ease-silk)",
        spring: "var(--ease-spring)",
      },
      transitionDuration: {
        fast: "var(--duration-fast)",
        normal: "var(--duration-normal)",
        slow: "var(--duration-slow)",
      },
    },
  },
  plugins: [],
};
