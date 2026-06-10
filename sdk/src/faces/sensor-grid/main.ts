import type { FaceContext, Rect, SensorAccessor } from '@hypercolor/sdk'
import {
    color,
    colorByValue,
    combo,
    face,
    font,
    grid,
    lerpColor,
    num,
    palette,
    rail,
    Smoothed,
    sensor,
    sensorColors,
    Timeline,
    toggle,
    withAlpha,
} from '@hypercolor/sdk'
import type { ChartPanel } from '../shared/components'
import { createChartPanel } from '../shared/components'

import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    humanizeSensorLabel,
    mixFaceAccent,
    resolveFaceInk,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-sensor-grid'

const STYLES = `
.hc-sensor-grid {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --value-size: 50;
    --label-size: 11;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
}

.hc-sensor-grid__frame {
    position: relative;
}

.hc-sensor-grid__cards {
    position: relative;
    width: 100%;
    height: 100%;
}

.hc-sensor-grid__card {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: 8px;
    box-sizing: border-box;
    background: transparent;
    border: none;
    will-change: transform, opacity;
}

.hc-sensor-grid__spark {
    position: absolute;
    inset: 12% 6% 12% 6%;
    opacity: 0.5;
    pointer-events: none;
}

.hc-sensor-grid__card-inner {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: center;
    text-align: center;
    width: 100%;
}


.hc-sensor-grid__label {
    font-family: var(--ui-font);
    font-size: calc(var(--label-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    text-align: center;
    color: var(--ui-ink);
}

.hc-sensor-grid__value {
    font-family: var(--hero-font);
    font-size: calc(var(--value-size) * 1px);
    font-weight: 600;
    line-height: 0.9;
    letter-spacing: 0.015em;
    text-align: center;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    text-shadow:
        0 0 18px color-mix(in srgb, var(--accent) 12%, transparent),
        0 8px 24px rgba(0,0,0,0.24);
}

.hc-sensor-grid__percent {
    font-family: var(--ui-font);
    font-size: calc(var(--label-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--dim-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
}

.hc-sensor-grid__track {
    position: relative;
    width: 80%;
    height: 6px;
    border-radius: 999px;
    overflow: hidden;
    background: rgba(255,255,255,0.06);
    justify-self: center;
}

.hc-sensor-grid__track-fill {
    position: absolute;
    inset: 0 auto 0 0;
    width: calc(var(--fill, 0) * 100%);
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), var(--secondary));
}

.hc-sensor-grid__hidden {
    display: none !important;
}
`

export default face(
    'Sensor Grid',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        colorMode: combo('Colors', ['Auto', 'Accent'], { group: 'Style' }),
        heroFont: font('Hero Font', 'Rajdhani', { families: [...DISPLAY_FONT_FAMILIES], group: 'Typography' }),
        labelSize: num('Label Size', [9, 20], 11, { group: 'Typography' }),
        sensor1: sensor('Top Left', 'cpu_temp', { group: 'Sensors' }),
        sensor2: sensor('Top Right', 'gpu_temp', { group: 'Sensors' }),
        sensor3: sensor('Bottom Left', 'cpu_load', { group: 'Sensors' }),
        sensor4: sensor('Bottom Right', 'ram_used', { group: 'Sensors' }),
        showLabels: toggle('Show Labels', true, { group: 'Elements' }),
        showPercents: toggle('Show Percents', false, { group: 'Elements' }),
        showSparklines: toggle('Show Sparklines', false, { group: 'Elements' }),
        showTracks: toggle('Show Tracks', true, { group: 'Elements' }),
        showValues: toggle('Show Values', true, { group: 'Elements' }),
        uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography' }),
        valueSize: num('Value Size', [28, 84], 50, { group: 'Typography' }),
    },
    {
        author: 'Hypercolor',
        description: 'A readable four-panel dashboard. Every element is independently toggleable.',
        designBasis: { height: 480, width: 480 },
        presets: [
            {
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Rajdhani',
                    sensor1: 'cpu_temp',
                    sensor2: 'gpu_temp',
                    sensor3: 'cpu_load',
                    sensor4: 'ram_used',
                    uiFont: 'Inter',
                },
                description: 'Balanced cyan dashboard for CPU, GPU, load, and memory.',
                name: 'System Vitals',
            },
            {
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Roboto Condensed',
                    sensor1: 'cpu_temp',
                    sensor2: 'gpu_temp',
                    sensor3: 'cpu_temp',
                    sensor4: 'gpu_temp',
                    uiFont: 'Inter',
                },
                description: 'All-temperature layout with compact condensed numerals.',
                name: 'Thermal Club',
            },
            {
                controls: {
                    accent: '#9ae7ff',
                    colorMode: 'Accent',
                    heroFont: 'Exo 2',
                    uiFont: 'Inter',
                },
                description: 'Cool blue accent with airy type.',
                name: 'Arctic Rail',
            },
            {
                controls: {
                    accent: palette.coral,
                    colorMode: 'Accent',
                    heroFont: 'Exo 2',
                    uiFont: 'DM Sans',
                },
                description: 'Coral matrix with softer, clearer hierarchy.',
                name: 'Signal Pink',
            },
            {
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                },
                description: 'Sharp monospaced telemetry.',
                name: 'Mono Ops',
            },
            {
                controls: {
                    accent: palette.electricYellow,
                    colorMode: 'Accent',
                    heroFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                },
                description: 'Warm gold accent with restrained chrome.',
                name: 'Gold Deck',
            },
            {
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Rajdhani',
                    showLabels: false,
                    showPercents: false,
                    showTracks: false,
                    uiFont: 'Inter',
                },
                description: 'Just the big values, no labels or chrome.',
                name: 'Naked Numbers',
            },
            {
                controls: {
                    accent: '#ffb45f',
                    colorMode: 'Accent',
                    heroFont: 'Roboto Condensed',
                    uiFont: 'Space Grotesk',
                },
                description: 'Warm amber survey deck with centered readings.',
                name: 'Amber Atlas',
            },
        ],
        variants: {
            wide: (ctx: FaceContext) => buildSensorGrid(ctx, 'rail'),
        },
    },
    (ctx) => buildSensorGrid(ctx, 'grid'),
)

// ── Shared implementation ───────────────────────────────────────────────

const ENTRANCE_STAGGER = 0.12
const ENTRANCE_DURATION = 0.5

type GridLayoutMode = 'grid' | 'rail'

function buildSensorGrid(ctx: FaceContext, mode: GridLayoutMode) {
    ensureFaceStyles(STYLE_ID, STYLES)
    const root = createFaceRoot(ctx, 'hc-sensor-grid')
    root.innerHTML = `
        <div class="hc-sensor-grid__frame">
            <div class="hc-sensor-grid__cards">
                ${Array.from(
                    { length: 4 },
                    () => `
                    <div class="hc-sensor-grid__card">
                        <div class="hc-sensor-grid__card-inner">
                            <div class="hc-sensor-grid__label">UNASSIGNED</div>
                            <div class="hc-sensor-grid__value">--</div>
                            <div class="hc-sensor-grid__percent">0%</div>
                            <div class="hc-sensor-grid__track"><div class="hc-sensor-grid__track-fill"></div></div>
                        </div>
                    </div>
                `,
                ).join('')}
            </div>
        </div>
    `

    const frameEl = root.querySelector<HTMLDivElement>('.hc-sensor-grid__frame')
    const cards = Array.from(root.querySelectorAll<HTMLDivElement>('.hc-sensor-grid__card'))
    if (!frameEl || cards.length !== 4) throw new Error('sensor-grid DOM failed to build')

    // Cells come from the layout module over the device safe area, so the
    // 2x2 stays inside a round panel and the rail spans the whole strip.
    const safe = ctx.display.safeArea
    const gap = Math.max(8, Math.round(Math.min(safe.width, safe.height) * 0.03))
    const area: Rect = { height: safe.height, width: safe.width, x: 0, y: 0 }
    const cells = mode === 'rail' ? rail(area, 4, gap) : grid(area, 2, 2, gap)
    frameEl.style.position = 'absolute'
    frameEl.style.left = `${safe.x}px`
    frameEl.style.top = `${safe.y}px`
    frameEl.style.width = `${safe.width}px`
    frameEl.style.height = `${safe.height}px`
    cards.forEach((card, index) => {
        const cell = cells[index]
        if (!cell) return
        card.style.left = `${cell.x}px`
        card.style.top = `${cell.y}px`
        card.style.width = `${cell.width}px`
        card.style.height = `${cell.height}px`
    })

    const sensorKeys = ['sensor1', 'sensor2', 'sensor3', 'sensor4'] as const
    const smoothValues = sensorKeys.map(() => new Smoothed(0, 0.3))
    const charts: Array<ChartPanel | null> = sensorKeys.map(() => null)
    const entrance = new Timeline()
    sensorKeys.forEach((_, index) => {
        entrance.add(`card${index}`, index * ENTRANCE_STAGGER, ENTRANCE_DURATION)
    })
    let appearedAt = Number.NaN
    let lastTime = Number.NaN
    let lastHistoryPush = 0

    return (time: number, controls: Record<string, unknown>, sensors: SensorAccessor) => {
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time
        if (Number.isNaN(appearedAt)) appearedAt = time
        const sinceAppear = time - appearedAt

        const colorMode = controls.colorMode as string
        const accent = lerpColor(controls.accent as string, palette.fg.primary, 0.04)
        const secondary = mixFaceAccent(accent)
        const ink = resolveFaceInk(accent)

        root.style.setProperty('--accent', accent)
        root.style.setProperty('--secondary', secondary)
        root.style.setProperty('--hero-ink', ink.hero)
        root.style.setProperty('--ui-ink', ink.ui)
        root.style.setProperty('--dim-ink', ink.dim)
        root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
        root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
        const valueScale = mode === 'rail' ? 0.72 : 1
        root.style.setProperty('--value-size', `${(controls.valueSize as number) * valueScale}`)
        root.style.setProperty('--label-size', `${controls.labelSize as number}`)

        const showLabels = controls.showLabels as boolean
        const showValues = controls.showValues as boolean
        const showPercents = controls.showPercents as boolean
        const showTracks = controls.showTracks as boolean
        const showSparklines = controls.showSparklines as boolean
        const pushHistory = time - lastHistoryPush > 0.25
        if (pushHistory) lastHistoryPush = time

        cards.forEach((card, index) => {
            const sensorLabel = controls[sensorKeys[index]] as string
            const reading = sensors.read(sensorLabel)
            const rawValue = sensors.normalized(sensorLabel)
            const value = smoothValues[index].update(rawValue, dt)

            const baseColor =
                colorMode === 'Auto'
                    ? reading?.unit === '°C' || reading?.unit === '°F'
                        ? colorByValue(value, sensorColors.temperature.gradient)
                        : reading?.unit === 'MB'
                          ? colorByValue(value, sensorColors.memory.gradient)
                          : colorByValue(value, sensorColors.load.gradient)
                    : accent
            const cardColor = lerpColor(baseColor, palette.fg.primary, 0.04)
            const cardSecondary = mixFaceAccent(cardColor, secondary, 0.32)
            const cardInk = resolveFaceInk(cardColor)

            card.style.setProperty('--accent', cardColor)
            card.style.setProperty('--secondary', cardSecondary)
            card.style.setProperty('--hero-ink', cardInk.hero)
            card.style.setProperty('--ui-ink', cardInk.ui)
            card.style.setProperty('--dim-ink', cardInk.dim)

            // Staggered entrance: cards rise and fade in one after another.
            const progress = entrance.progress(`card${index}`, sinceAppear)
            if (progress < 1) {
                card.style.opacity = `${progress}`
                card.style.transform = `translateY(${(1 - progress) * 14}px)`
            } else {
                card.style.opacity = '1'
                card.style.transform = 'translateY(0)'
            }

            const labelEl = card.querySelector<HTMLElement>('.hc-sensor-grid__label')
            const valueEl = card.querySelector<HTMLElement>('.hc-sensor-grid__value')
            const percentEl = card.querySelector<HTMLElement>('.hc-sensor-grid__percent')
            const trackEl = card.querySelector<HTMLElement>('.hc-sensor-grid__track')
            const fillEl = card.querySelector<HTMLElement>('.hc-sensor-grid__track-fill')
            if (!labelEl || !valueEl || !percentEl || !trackEl || !fillEl) return

            valueEl.textContent = sensors.formatted(sensorLabel)
            labelEl.textContent = humanizeSensorLabel(sensorLabel)
            percentEl.textContent = `${Math.round(clamp01(value) * 100)}%`
            fillEl.style.setProperty('--fill', clamp01(value).toFixed(4))

            labelEl.classList.toggle('hc-sensor-grid__hidden', !showLabels)
            valueEl.classList.toggle('hc-sensor-grid__hidden', !showValues)
            percentEl.classList.toggle('hc-sensor-grid__hidden', !showPercents)
            trackEl.classList.toggle('hc-sensor-grid__hidden', !showTracks)

            if (showSparklines) {
                if (!charts[index]) {
                    const panel = createChartPanel(card, {
                        capacity: 48,
                        color: withAlpha(cardColor, 0.6),
                        range: [0, 1],
                    })
                    panel.element.className = 'hc-sensor-grid__spark'
                    const cell = cells[index]
                    panel.resize((cell?.width ?? 100) * 0.88, (cell?.height ?? 100) * 0.6)
                    charts[index] = panel
                }
                const panel = charts[index]
                if (panel) {
                    if (pushHistory) panel.push(clamp01(value))
                    panel.draw()
                    panel.element.style.display = ''
                }
            } else if (charts[index]) {
                charts[index]?.element.style.setProperty('display', 'none')
            }
        })

        const c = ctx.ctx
        c.clearRect(0, 0, ctx.width, ctx.height)
    }
}
