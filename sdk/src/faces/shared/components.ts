/**
 * Face component library — DOM builders that own their animation state.
 *
 * Components are positioned into layout-module cells via `place(rect)` and
 * advanced once per frame from the face update function. All motion runs
 * through the SDK motion primitives, so it stays frame-rate independent.
 *
 * Lives in the faces workspace; promote to `packages/core` if a third
 * consumer outside the bundled faces appears.
 */

import type { Rect, SparklineBand } from '@hypercolor/sdk'
import { palette, Smoothed, sparkline, ValueHistory, withAlpha } from '@hypercolor/sdk'

// ── Shared plumbing ─────────────────────────────────────────────────────

function placeInto(element: HTMLElement, rect: Rect): void {
    element.style.position = 'absolute'
    element.style.left = `${rect.x}px`
    element.style.top = `${rect.y}px`
    element.style.width = `${rect.width}px`
    element.style.height = `${rect.height}px`
}

function el<K extends keyof HTMLElementTagNameMap>(
    tag: K,
    parent: HTMLElement,
    className: string,
): HTMLElementTagNameMap[K] {
    const node = document.createElement(tag)
    node.className = className
    parent.appendChild(node)
    return node
}

// ── Readout ─────────────────────────────────────────────────────────────

export interface ReadoutOptions {
    label: string
    accent?: string
    /** Value font size in px (default: 28). */
    valueSize?: number
    /** Label font size in px (default: 10). */
    labelSize?: number
}

export interface Readout {
    element: HTMLDivElement
    place(rect: Rect): void
    update(value: string): void
    setLabel(label: string): void
    setAccent(color: string): void
}

/** Label + value pair with tabular numerals. */
export function createReadout(parent: HTMLElement, options: ReadoutOptions): Readout {
    const accent = options.accent ?? palette.neonCyan
    const root = el('div', parent, 'hc-readout')
    root.style.display = 'flex'
    root.style.flexDirection = 'column'
    root.style.justifyContent = 'center'
    root.style.gap = '2px'
    root.style.overflow = 'hidden'

    const label = el('div', root, 'hc-readout-label')
    label.style.fontSize = `${options.labelSize ?? 10}px`
    // Faces publish their font controls as CSS vars on the root; without
    // an explicit family the cards inherit the browser's serif default.
    label.style.fontFamily = "var(--ui-font, 'Inter', sans-serif)"
    label.style.fontWeight = '600'
    label.style.letterSpacing = '0.14em'
    label.style.textTransform = 'uppercase'
    label.style.color = withAlpha(accent, 0.7)
    label.textContent = options.label

    const value = el('div', root, 'hc-readout-value')
    value.style.fontSize = `${options.valueSize ?? 28}px`
    value.style.fontFamily = "var(--hero-font, 'Rajdhani', sans-serif)"
    value.style.fontWeight = '600'
    value.style.fontVariantNumeric = 'tabular-nums'
    value.style.color = palette.fg.primary
    value.textContent = '--'

    return {
        element: root,
        place: (rect) => placeInto(root, rect),
        setAccent(color) {
            label.style.color = withAlpha(color, 0.7)
        },
        setLabel(text) {
            label.textContent = text
        },
        update(text) {
            if (value.textContent !== text) value.textContent = text
        },
    }
}

// ── ProgressBar ─────────────────────────────────────────────────────────

export interface ProgressBarOptions {
    accent?: string
    /** Track height in px (default: 6). */
    height?: number
    /** Seconds to close half the distance to a new value (default: 0.12). */
    halflife?: number
}

export interface ProgressBar {
    element: HTMLDivElement
    place(rect: Rect): void
    /** Advance toward `normalized` (0–1) by `dt` seconds. */
    update(normalized: number, dt: number): void
    /** Current eased fill 0–1. */
    value(): number
    setAccent(color: string): void
}

/** Animated horizontal fill bar. */
export function createProgressBar(parent: HTMLElement, options: ProgressBarOptions = {}): ProgressBar {
    const accent = options.accent ?? palette.neonCyan
    const height = options.height ?? 6

    const track = el('div', parent, 'hc-progress')
    track.style.background = withAlpha(palette.fg.primary, 0.08)
    track.style.borderRadius = `${height / 2}px`
    track.style.overflow = 'hidden'
    track.style.height = `${height}px`

    const fill = el('div', track, 'hc-progress-fill')
    fill.style.height = '100%'
    fill.style.width = '0%'
    fill.style.borderRadius = 'inherit'
    fill.style.background = accent
    fill.style.boxShadow = `0 0 ${height * 1.5}px ${withAlpha(accent, 0.55)}`

    const eased = new Smoothed(0, options.halflife ?? 0.12)
    let lastPercent = -1

    return {
        element: track,
        place(rect) {
            placeInto(track, { ...rect, height, y: rect.y + (rect.height - height) / 2 })
        },
        setAccent(color) {
            fill.style.background = color
            fill.style.boxShadow = `0 0 ${height * 1.5}px ${withAlpha(color, 0.55)}`
        },
        update(normalized, dt) {
            const next = eased.update(Math.max(0, Math.min(1, normalized)), dt)
            const percent = Math.round(next * 1000) / 10
            if (percent !== lastPercent) {
                lastPercent = percent
                fill.style.width = `${percent}%`
            }
        },
        value: () => eased.value,
    }
}

// ── MetricCard ──────────────────────────────────────────────────────────

export interface MetricCardOptions {
    label: string
    accent?: string
    /** Show the animated fill bar under the value (default: true). */
    bar?: boolean
    /** Per-card rolling sparkline behind the value (default: false). */
    sparkline?: boolean
    /** Sparkline history length (default: 48). */
    historyLength?: number
    /** Bar smoothing half-life in seconds (default: 0.12). */
    halflife?: number
}

export interface MetricCardUpdate {
    /** Formatted display string (e.g. "64°C"). */
    text: string
    /** Normalized 0–1 value driving the bar and sparkline. */
    normalized: number
    /** Frame delta in seconds. */
    dt: number
}

export interface MetricCard {
    element: HTMLDivElement
    place(rect: Rect): void
    update(update: MetricCardUpdate): void
    setLabel(label: string): void
    /** Recolor the border, label, bar, and sparkline. */
    setAccent(color: string): void
    /** Current eased bar fill 0–1. */
    barValue(): number
}

/** Label, value, animated bar, and optional sparkline in one card. */
export function createMetricCard(parent: HTMLElement, options: MetricCardOptions): MetricCard {
    const accent = options.accent ?? palette.neonCyan
    const root = el('div', parent, 'hc-metric-card')
    root.style.display = 'flex'
    root.style.flexDirection = 'column'
    root.style.justifyContent = 'center'
    root.style.gap = '4px'
    root.style.padding = '8px 10px'
    root.style.boxSizing = 'border-box'
    root.style.borderRadius = '10px'
    root.style.background = withAlpha(palette.fg.primary, 0.05)
    root.style.border = `1px solid ${withAlpha(accent, 0.18)}`
    root.style.overflow = 'hidden'

    let chart: ChartPanel | null = null
    if (options.sparkline) {
        chart = createChartPanel(root, {
            capacity: options.historyLength ?? 48,
            color: withAlpha(accent, 0.5),
            range: [0, 1],
        })
        chart.element.style.position = 'absolute'
        chart.element.style.inset = '0'
        chart.element.style.opacity = '0.6'
    }

    const readout = createReadout(root, { accent, label: options.label })
    readout.element.style.position = 'relative'

    let bar: ProgressBar | null = null
    if (options.bar !== false) {
        bar = createProgressBar(root, { accent, halflife: options.halflife })
        bar.element.style.position = 'relative'
    }

    return {
        barValue: () => bar?.value() ?? 0,
        element: root,
        place(rect) {
            placeInto(root, rect)
            chart?.resize(rect.width, rect.height)
        },
        setAccent(color) {
            root.style.border = `1px solid ${withAlpha(color, 0.18)}`
            readout.setAccent(color)
            bar?.setAccent(color)
            chart?.setColor(withAlpha(color, 0.5))
        },
        setLabel: (label) => readout.setLabel(label),
        update({ text, normalized, dt }) {
            readout.update(text)
            bar?.update(normalized, dt)
            if (chart) {
                chart.push(normalized)
                chart.draw()
            }
        },
    }
}

// ── ChartPanel ──────────────────────────────────────────────────────────

export interface ChartPanelOptions {
    color: string
    range: [number, number]
    /** Rolling history length (default: 60). */
    capacity?: number
    /** Threshold color zones for the line. */
    bands?: SparklineBand[]
    /** Reveal fraction 0–1 for animated draw-in (default: 1). */
    drawIn?: number
}

export interface ChartPanel {
    element: HTMLCanvasElement
    place(rect: Rect): void
    resize(width: number, height: number): void
    push(value: number): void
    /** Render the current history; `drawIn` overrides the option. */
    draw(drawIn?: number): void
    history: ValueHistory
    setColor(color: string): void
}

/** Canvas-backed rolling chart built on the SDK sparkline. */
export function createChartPanel(parent: HTMLElement, options: ChartPanelOptions): ChartPanel {
    const canvas = el('canvas', parent, 'hc-chart-panel')
    canvas.width = 1
    canvas.height = 1
    const history = new ValueHistory(options.capacity ?? 60)
    let lineColor = options.color

    const panel: ChartPanel = {
        draw(drawIn) {
            const ctx = canvas.getContext('2d')
            if (!ctx || history.length < 2) return
            ctx.clearRect(0, 0, canvas.width, canvas.height)
            sparkline(ctx, {
                bands: options.bands,
                color: lineColor,
                drawIn: drawIn ?? options.drawIn ?? 1,
                height: canvas.height,
                range: options.range,
                values: history.values(),
                width: canvas.width,
                x: 0,
                y: 0,
            })
        },
        element: canvas,
        history,
        place(rect) {
            placeInto(canvas, rect)
            panel.resize(rect.width, rect.height)
        },
        push: (value) => history.push(value),
        resize(width, height) {
            const w = Math.max(Math.round(width), 1)
            const h = Math.max(Math.round(height), 1)
            if (canvas.width !== w) canvas.width = w
            if (canvas.height !== h) canvas.height = h
        },
        setColor(color) {
            lineColor = color
        },
    }
    return panel
}
