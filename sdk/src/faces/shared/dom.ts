import type { FaceContext } from 'hypercolor'
import { lerpColor, palette, parseHex, Smoothed, withAlpha } from 'hypercolor'

export const DISPLAY_FONT_FAMILIES = [
    'Orbitron',
    'Audiowide',
    'Bebas Neue',
    'Exo 2',
    'Rajdhani',
    'Space Mono',
    'JetBrains Mono',
] as const

export const UI_FONT_FAMILIES = ['Sora', 'Space Grotesk', 'DM Sans', 'Inter', 'Roboto Condensed', 'Exo 2'] as const

export function ensureFaceStyles(id: string, css: string): void {
    if (document.getElementById(id)) return
    const style = document.createElement('style')
    style.id = id
    style.textContent = css
    document.head.appendChild(style)
}

export function createFaceRoot(ctx: FaceContext, className: string): HTMLDivElement {
    const existing = ctx.container.querySelector<HTMLDivElement>(`:scope > .${className}`)
    if (existing) {
        existing.innerHTML = ''
        existing.style.background = 'transparent'
        return existing
    }

    const root = document.createElement('div')
    root.className = className
    root.style.position = 'absolute'
    root.style.inset = '0'
    root.style.zIndex = '3'
    root.style.pointerEvents = 'none'
    // Flex, not grid: Servo renders flexbox but silently ignores grid
    // (see the css-probe matrix in hypercolor-core).
    root.style.display = 'flex'
    root.style.alignItems = 'center'
    root.style.justifyContent = 'center'
    root.style.width = '100%'
    root.style.height = '100%'
    root.style.background = 'transparent'
    ctx.container.appendChild(root)
    return root
}

export function humanizeSensorLabel(label: string): string {
    if (!label) return 'Unassigned'
    return label.replace(/[_-]+/g, ' ').replace(/\b\w/g, (match) => match.toUpperCase())
}

export function clamp01(value: number): number {
    return Math.max(0, Math.min(1, value))
}

const GLIDABLE_HEX = /^#[0-9a-f]{6}$/i

/**
 * Frame-rate-independent glide between hex colors, so control and preset
 * changes sweep instead of snapping. The first update adopts its target
 * directly (no boot wash from the constructor seed), and anything that
 * isn't 6-digit hex (hsl strings from audio-driven palettes) passes
 * through and resets the glide.
 */
export class SmoothedColor {
    private readonly r: Smoothed
    private readonly g: Smoothed
    private readonly b: Smoothed
    private initialized = false

    /** @param halflife seconds to close half the distance (default 0.1). */
    constructor(initial: string, halflife = 0.1) {
        const [r, g, b] = parseHex(GLIDABLE_HEX.test(initial) ? initial : '#ffffff')
        this.r = new Smoothed(r, halflife)
        this.g = new Smoothed(g, halflife)
        this.b = new Smoothed(b, halflife)
    }

    /** Advance toward `target` by `dt` seconds; returns the glided color. */
    update(target: string, dt: number): string {
        if (!GLIDABLE_HEX.test(target)) {
            this.initialized = false
            return target
        }
        const [r, g, b] = parseHex(target)
        if (!this.initialized) {
            this.initialized = true
            this.r.snap(r)
            this.g.snap(g)
            this.b.snap(b)
            return target
        }
        const channel = (value: number) =>
            Math.round(Math.max(0, Math.min(255, value)))
                .toString(16)
                .padStart(2, '0')
        return `#${channel(this.r.update(r, dt))}${channel(this.g.update(g, dt))}${channel(this.b.update(b, dt))}`
    }
}

export interface FaceInk {
    hero: string
    ui: string
    dim: string
    edge: string
    glow: string
}

export function mixFaceAccent(base: string, target: string = palette.electricPurple, amount = 0.42): string {
    return lerpColor(base, target, clamp01(amount))
}

export function resolveFaceInk(accent: string): FaceInk {
    const hero = lerpColor(accent, palette.fg.primary, 0.22)
    const ui = lerpColor(accent, palette.fg.secondary, 0.46)
    const dim = lerpColor(accent, palette.fg.tertiary, 0.68)
    return {
        dim,
        edge: withAlpha(lerpColor(accent, palette.fg.secondary, 0.58), 0.24),
        glow: withAlpha(accent, 0.24),
        hero,
        ui,
    }
}
