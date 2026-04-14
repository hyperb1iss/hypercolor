/**
 * SilkCircuit design tokens for display faces.
 *
 * Importable constants matching the SilkCircuit design language — palette,
 * spacing, typography, and sensor-specific color schemes for gauges.
 */

// ── Core Palette ───────────────────────────────────────────────────────

export const palette = {
    bg: {
        deep: '#0a0a12',
        overlay: '#1a1a2e',
        raised: '#242440',
        surface: '#12121f',
    },
    coral: '#ff6ac1',
    electricPurple: '#e135ff',
    electricYellow: '#f1fa8c',
    errorRed: '#ff6363',

    fg: {
        primary: '#e8e6f0',
        secondary: '#9d9bb0',
        tertiary: '#6b6980',
    },
    neonCyan: '#80ffea',
    successGreen: '#50fa7b',
} as const

// ── Spacing Scale ──────────────────────────────────────────────────────

export const spacing = {
    lg: 24,
    md: 16,
    sm: 8,
    xl: 32,
    xs: 4,
    xxl: 48,
} as const

// ── Border Radius ──────────────────────────────────────────────────────

export const radius = {
    full: 9999,
    lg: 16,
    md: 8,
    sm: 4,
} as const

// ── Sensor Color Schemes ───────────────────────────────────────────────

export const sensorColors = {
    load: {
        gradient: ['#50fa7b', '#f1fa8c', '#ff6ac1'] as const,
        high: '#ff6ac1' as const,
        low: '#50fa7b' as const,
        mid: '#f1fa8c' as const,
    },
    memory: {
        free: '#80ffea' as const,
        gradient: ['#80ffea', '#e135ff'] as const,
        used: '#e135ff' as const,
    },
    temperature: {
        cool: '#80ffea' as const,
        gradient: ['#80ffea', '#f1fa8c', '#ff6363'] as const,
        hot: '#ff6363' as const,
        warm: '#f1fa8c' as const,
    },
} as const

// ── Color Utilities ────────────────────────────────────────────────────

/** Parse a hex color (#RGB, #RRGGBB, or #RRGGBBAA) to [r, g, b, a] 0–255. */
export function parseHex(hex: string): [number, number, number, number] {
    const h = hex.replace('#', '')
    if (h.length === 3) {
        return [
            Number.parseInt(h[0] + h[0], 16),
            Number.parseInt(h[1] + h[1], 16),
            Number.parseInt(h[2] + h[2], 16),
            255,
        ]
    }
    if (h.length === 6) {
        return [
            Number.parseInt(h.slice(0, 2), 16),
            Number.parseInt(h.slice(2, 4), 16),
            Number.parseInt(h.slice(4, 6), 16),
            255,
        ]
    }
    return [
        Number.parseInt(h.slice(0, 2), 16),
        Number.parseInt(h.slice(2, 4), 16),
        Number.parseInt(h.slice(4, 6), 16),
        Number.parseInt(h.slice(6, 8), 16),
    ]
}

/** Linearly interpolate between two hex colors by ratio [0–1]. */
export function lerpColor(a: string, b: string, t: number): string {
    const [ar, ag, ab] = parseHex(a)
    const [br, bg, bb] = parseHex(b)
    const clamped = Math.max(0, Math.min(1, t))
    const r = Math.round(ar + (br - ar) * clamped)
    const g = Math.round(ag + (bg - ag) * clamped)
    const bl = Math.round(ab + (bb - ab) * clamped)
    return `#${r.toString(16).padStart(2, '0')}${g.toString(16).padStart(2, '0')}${bl.toString(16).padStart(2, '0')}`
}

/**
 * Pick a color from a multi-stop gradient based on a 0–1 value.
 *
 * @example
 * ```typescript
 * colorByValue(0.8, sensorColors.temperature.gradient) // warm-to-hot
 * ```
 */
export function colorByValue(value: number, stops: readonly string[]): string {
    if (stops.length === 0) return '#ffffff'
    if (stops.length === 1) return stops[0]
    const clamped = Math.max(0, Math.min(1, value))
    const segment = clamped * (stops.length - 1)
    const i = Math.min(Math.floor(segment), stops.length - 2)
    const localT = segment - i
    return lerpColor(stops[i], stops[i + 1], localT)
}

/** Apply an alpha channel to a hex color and return an rgba() CSS string. */
export function withAlpha(color: string, alpha: number): string {
    const [r, g, b] = parseHex(color)
    const clamped = Math.max(0, Math.min(1, alpha))
    return `rgba(${r}, ${g}, ${b}, ${clamped})`
}

/** Apply a neon glow (shadowBlur) around subsequent canvas draws. */
export function withGlow(ctx: CanvasRenderingContext2D, color: string, intensity: number, fn: () => void): void {
    if (intensity <= 0) {
        fn()
        return
    }
    const prevBlur = ctx.shadowBlur
    const prevColor = ctx.shadowColor
    ctx.shadowBlur = intensity * 20
    ctx.shadowColor = color
    fn()
    ctx.shadowBlur = prevBlur
    ctx.shadowColor = prevColor
}
