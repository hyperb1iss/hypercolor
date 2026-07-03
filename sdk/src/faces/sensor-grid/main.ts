import type { FaceContext } from '@hypercolor/sdk'
import {
    color,
    combo,
    easeOutCubic,
    face,
    font,
    lerpColor,
    num,
    palette,
    Smoothed,
    sensor,
    toggle,
    ValueHistory,
    withAlpha,
} from '@hypercolor/sdk'

import {
    atmosphereVisible,
    drawNebulaField,
    drawRisingMotes,
    entrance,
    makeDrifters,
    transparentBackgroundControl,
} from '../shared/atmosphere'
import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    humanizeSensorLabel,
    resolveFaceInk,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-sensor-grid'
const HISTORY_PUSH_INTERVAL = 0.6

/** Per-domain heat ramps for Auto color mode. */
function autoRamp(label: string): [string, string] {
    const key = label.toLowerCase()
    if (key.includes('temp')) return ['#37e0ff', '#ff5e7a']
    if (key.includes('load') || key.includes('usage')) return ['#5a8dff', palette.electricYellow]
    if (key.includes('ram') || key.includes('mem')) return [palette.electricPurple, palette.coral]
    return [palette.neonCyan, palette.electricPurple]
}

const STYLES = `
.hc-sensor-grid {
    --accent: ${palette.neonCyan};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --value-size: 50;
    --label-size: 11;
    position: absolute;
    inset: 0;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--hero-ink);
}

.hc-sensor-grid__cells {
    position: relative;
    z-index: 2;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
}

.hc-sensor-grid__cell {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    text-align: center;
    will-change: transform, opacity;
}

.hc-sensor-grid__label {
    font-family: var(--ui-font);
    font-size: calc(var(--label-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.26em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-sensor-grid__value {
    display: inline-flex;
    align-items: flex-start;
    font-family: var(--hero-font);
    font-size: calc(var(--value-size) * 1px);
    font-weight: 400;
    line-height: 0.86;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    text-shadow: 0 0 26px color-mix(in srgb, var(--cell-heat, var(--accent)) 45%, transparent);
}

.hc-sensor-grid__unit {
    font-size: 0.42em;
    font-weight: 500;
    margin-top: 0.16em;
    margin-left: 0.08em;
    color: color-mix(in srgb, var(--hero-ink) 55%, var(--cell-heat, var(--accent)));
}

.hc-sensor-grid__hidden { display: none !important; }
`

export default face(
    'Sensor Grid',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        colorMode: combo('Colors', ['Auto', 'Accent'], { group: 'Style' }),
        heroFont: font('Hero Font', 'Rajdhani', {
            families: [...DISPLAY_FONT_FAMILIES],
            group: 'Typography',
            weights: [400, 500],
        }),
        labelSize: num('Label Size', [9, 20], 11, { group: 'Typography' }),
        sensor1: sensor('Top Left', 'cpu_temp', { group: 'Sensors' }),
        sensor2: sensor('Top Right', 'gpu_temp', { group: 'Sensors' }),
        sensor3: sensor('Bottom Left', 'cpu_load', { group: 'Sensors' }),
        sensor4: sensor('Bottom Right', 'ram_used', { group: 'Sensors' }),
        showLabels: toggle('Show Labels', true, { group: 'Elements' }),
        showPercents: toggle('Show Percents', false, { group: 'Elements' }),
        showSparklines: toggle('Show Sparklines', true, { group: 'Elements' }),
        showTracks: toggle('Show Tracks', true, { group: 'Elements' }),
        showValues: toggle('Show Values', true, { group: 'Elements' }),
        transparentBackground: transparentBackgroundControl(),
        uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography', weights: [600] }),
        valueSize: num('Value Size', [28, 84], 50, { group: 'Typography' }),
    },
    {
        author: 'Hypercolor',
        description:
            'Four readings as living energy cells: each value breathes its own heat glow over a shared nebula.',
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
            wide: (ctx: FaceContext) => buildSensorGrid(ctx, true),
        },
    },
    (ctx) => buildSensorGrid(ctx, false),
)

interface CellRuntime {
    root: HTMLDivElement
    labelEl: HTMLElement
    valueEl: HTMLElement
    digitsEl: HTMLElement
    unitEl: HTMLElement
    heat: Smoothed
    shown: Smoothed
    history: ValueHistory
    lastPush: number
}

function buildSensorGrid(ctx: FaceContext, wide: boolean) {
    ensureFaceStyles(STYLE_ID, STYLES)
    const root = createFaceRoot(ctx, 'hc-sensor-grid')
    const cellsEl = document.createElement('div')
    cellsEl.className = 'hc-sensor-grid__cells'
    root.appendChild(cellsEl)

    const safe = ctx.display.safeArea
    const gridW = wide ? ctx.width * 0.94 : safe.width
    const gridH = wide ? ctx.height * 0.92 : safe.height
    cellsEl.style.width = `${gridW}px`
    cellsEl.style.height = `${gridH}px`

    const cellW = wide ? gridW / 4 : gridW / 2
    const cellH = wide ? gridH : gridH / 2

    const cells: CellRuntime[] = []
    for (let index = 0; index < 4; index += 1) {
        const cell = document.createElement('div')
        cell.className = 'hc-sensor-grid__cell'
        cell.style.width = `${cellW}px`
        cell.style.height = `${cellH}px`
        cell.innerHTML = `
            <div class="hc-sensor-grid__label">--</div>
            <div class="hc-sensor-grid__value"><span class="hc-sensor-grid__digits">--</span><span class="hc-sensor-grid__unit"></span></div>`
        cellsEl.appendChild(cell)
        const labelEl = cell.querySelector<HTMLElement>('.hc-sensor-grid__label')
        const valueEl = cell.querySelector<HTMLElement>('.hc-sensor-grid__value')
        const digitsEl = cell.querySelector<HTMLElement>('.hc-sensor-grid__digits')
        const unitEl = cell.querySelector<HTMLElement>('.hc-sensor-grid__unit')
        if (!labelEl || !valueEl || !digitsEl || !unitEl) {
            throw new Error('Sensor Grid failed to build its DOM')
        }
        cells.push({
            digitsEl,
            heat: new Smoothed(0, 0.8),
            history: new ValueHistory(60),
            labelEl,
            lastPush: Number.NEGATIVE_INFINITY,
            root: cell,
            shown: new Smoothed(0, 0.3),
            unitEl,
            valueEl,
        })
    }

    const drifters = makeDrifters(wide ? 30 : 18)
    const sensorKeys = ['sensor1', 'sensor2', 'sensor3', 'sensor4'] as const
    let bootAt = Number.NaN
    let lastTime = Number.NaN

    return (time: number, controls: Record<string, unknown>, sensors: import('@hypercolor/sdk').SensorAccessor) => {
        if (Number.isNaN(bootAt)) bootAt = time
        const boot = time - bootAt
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time
        const accent = controls.accent as string
        const ink = resolveFaceInk(accent)
        const auto = controls.colorMode !== 'Accent'

        root.style.setProperty('--accent', accent)
        root.style.setProperty('--hero-ink', ink.hero)
        root.style.setProperty('--ui-ink', ink.ui)
        root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
        root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
        const scaleBasis = wide ? (ctx.height / 480) * 1.7 : Math.min(safe.width, safe.height) / 339
        root.style.setProperty('--value-size', `${(controls.valueSize as number) * scaleBasis}`)
        root.style.setProperty(
            '--label-size',
            `${Math.max(9, (controls.labelSize as number) * Math.max(scaleBasis, 0.85))}`,
        )

        const c = ctx.ctx
        const W = ctx.width
        const H = ctx.height
        c.clearRect(0, 0, W, H)

        // Shared sky, tinted toward the hottest cell.
        let hottest = 0
        for (const cell of cells) hottest = Math.max(hottest, cell.heat.value)
        const skyColor = auto ? lerpColor('#37e0ff', '#ff5e7a', hottest) : accent
        if (atmosphereVisible(controls)) {
            drawNebulaField(c, W, H, time, skyColor, accent, 0.55 + hottest * 0.5)
            drawRisingMotes(c, W, H, time, drifters, skyColor, 0.6, hottest)
        }

        const gridLeft = (W - gridW) / 2
        const gridTop = (H - gridH) / 2

        for (let index = 0; index < 4; index += 1) {
            const cell = cells[index]
            const key = sensorKeys[index]
            if (!cell || !key) continue
            const label = controls[key] as string
            const reading = sensors.read(label)
            const normalized = clamp01(sensors.normalized(label))
            const heat = cell.heat.update(normalized, dt)
            const ramp = auto ? autoRamp(label) : ([withAlpha(accent, 0.55), accent] as [string, string])
            const heatColor = auto ? lerpColor(ramp[0], ramp[1], heat) : accent

            if (time - cell.lastPush >= HISTORY_PUSH_INTERVAL) {
                cell.lastPush = time
                cell.history.push(normalized)
            }

            // Staggered entrance, one beat per cell.
            const cellIn = entrance(boot, 0.12 + index * 0.14, 0.8)
            cell.root.style.opacity = `${cellIn}`
            cell.root.style.transform = `translateY(${(1 - cellIn) * 14}px)`
            cell.root.style.setProperty('--cell-heat', heatColor)

            cell.labelEl.textContent = humanizeSensorLabel(label).toUpperCase()
            const shown = cell.shown.update(reading?.value ?? 0, dt)
            const showPercent = controls.showPercents === true && reading?.unit === '%'
            cell.digitsEl.textContent = reading ? `${Math.round(shown)}` : '--'
            cell.unitEl.textContent = reading ? (showPercent || reading.unit !== '%' ? reading.unit : '') : ''
            cell.labelEl.classList.toggle('hc-sensor-grid__hidden', controls.showLabels !== true)
            cell.valueEl.classList.toggle('hc-sensor-grid__hidden', controls.showValues !== true)

            // ── Canvas layer per cell: heat glow, track, aurora ──
            const col = wide ? index : index % 2
            const row = wide ? 0 : Math.floor(index / 2)
            const cx = gridLeft + col * cellW + cellW / 2
            const cy = gridTop + row * cellH + cellH / 2
            const glowRadius = Math.min(cellW, cellH) * (0.52 + heat * 0.16)
            const glowGradient = c.createRadialGradient(cx, cy, 0, cx, cy, glowRadius)
            glowGradient.addColorStop(0, withAlpha(heatColor, (0.1 + heat * 0.16) * cellIn))
            glowGradient.addColorStop(1, withAlpha(heatColor, 0))
            c.fillStyle = glowGradient
            c.fillRect(cx - glowRadius, cy - glowRadius, glowRadius * 2, glowRadius * 2)

            if (controls.showTracks === true) {
                const trackW = cellW * 0.42
                const trackY = cy + cellH * 0.26
                c.strokeStyle = withAlpha('#8a8fa8', 0.18)
                c.lineWidth = 2
                c.beginPath()
                c.moveTo(cx - trackW / 2, trackY)
                c.lineTo(cx + trackW / 2, trackY)
                c.stroke()
                c.save()
                c.shadowColor = heatColor
                c.shadowBlur = 8
                c.strokeStyle = withAlpha(heatColor, 0.85 * cellIn)
                c.beginPath()
                c.moveTo(cx - trackW / 2, trackY)
                c.lineTo(cx - trackW / 2 + trackW * heat * easeOutCubic(cellIn), trackY)
                c.stroke()
                c.restore()
            }

            if (controls.showSparklines === true) {
                const values = cell.history.values()
                if (values.length > 1) {
                    const sparkW = cellW * 0.52
                    const sparkH = cellH * 0.14
                    const baseY = cy + cellH * 0.38
                    c.beginPath()
                    c.moveTo(cx - sparkW / 2, baseY)
                    for (let vi = 0; vi < values.length; vi += 1) {
                        const x = cx - sparkW / 2 + (vi / (values.length - 1)) * sparkW
                        c.lineTo(x, baseY - clamp01(values[vi] ?? 0) * sparkH)
                    }
                    c.lineTo(cx + sparkW / 2, baseY)
                    c.closePath()
                    const ribbon = c.createLinearGradient(0, baseY - sparkH, 0, baseY)
                    ribbon.addColorStop(0, withAlpha(heatColor, 0.3 * cellIn))
                    ribbon.addColorStop(1, withAlpha(heatColor, 0))
                    c.fillStyle = ribbon
                    c.fill()
                }
            }
        }
    }
}
