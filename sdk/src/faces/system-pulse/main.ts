import type { FaceContext, FaceDataSources } from '@hypercolor/sdk'
import { color, combo, face, font, palette, sensor, sparkline, toggle, ValueHistory, withAlpha } from '@hypercolor/sdk'
import {
    atmosphereVisible,
    drawNebulaField,
    drawRisingMotes,
    makeDrifters,
    transparentBackgroundControl,
} from '../shared/atmosphere'
import { createMetricCard, type MetricCard } from '../shared/components'
import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    resolveFaceInk,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-system-pulse'
const NET_HISTORY = 90
/** Floor for the rolling autoscale so idle traffic doesn't look dramatic. */
const NET_SCALE_FLOOR_BPS = 256 * 1024

const STYLES = `
.hc-pulse {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
    display: flex;
    align-items: center;
    justify-content: center;
}

.hc-pulse__stack {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 12px;
}

.hc-pulse__clock {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
}

.hc-pulse__time {
    font-family: var(--hero-font);
    font-weight: 600;
    line-height: 0.9;
    letter-spacing: 0.01em;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    text-shadow: 0 0 18px color-mix(in srgb, var(--accent) 14%, transparent);
}

.hc-pulse__date {
    font-family: var(--ui-font);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-pulse__cards {
    display: flex;
    flex-direction: row;
    gap: 10px;
}

.hc-pulse__cards > * {
    flex: 1 1 0;
    min-width: 0;
}

.hc-pulse__net {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 10px;
    border-radius: 10px;
    background: rgba(255,255,255,0.05);
    border: 1px solid color-mix(in srgb, var(--accent) 18%, transparent);
    overflow: hidden;
}

.hc-pulse__net-head {
    display: flex;
    flex-direction: row;
    justify-content: space-between;
    align-items: baseline;
    gap: 10px;
    font-family: var(--ui-font);
    font-weight: 600;
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    position: relative;
    z-index: 1;
}

.hc-pulse__net-label {
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-pulse__net-rates {
    display: flex;
    flex-direction: row;
    gap: 12px;
    color: var(--hero-ink);
}

.hc-pulse__net-canvas {
    position: absolute;
    inset: 0;
    z-index: 0;
    opacity: 0.85;
}

.hc-pulse__rig {
    display: flex;
    flex-direction: row;
    align-items: center;
    gap: 10px;
}

.hc-pulse__rig-scene {
    font-family: var(--ui-font);
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--dim-ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
}

.hc-pulse__rig-strip {
    flex: 1 1 auto;
    height: 5px;
    border-radius: 999px;
    background: rgba(255,255,255,0.06);
}

/* ── Wide strip layout ── */

.hc-pulse--wide .hc-pulse__stack {
    flex-direction: row;
    align-items: center;
    gap: 2.4%;
}

.hc-pulse--wide .hc-pulse__clock { flex: 0 0 auto; }
.hc-pulse--wide .hc-pulse__cards { flex: 1 1 auto; min-width: 0; }
.hc-pulse--wide .hc-pulse__side {
    display: flex;
    flex-direction: column;
    gap: 8px;
    flex: 0 0 30%;
    min-width: 0;
}

.hc-pulse__hidden { display: none !important; }
`

function formatBps(bps: number): string {
    if (bps >= 1_000_000_000) return `${(bps / 1_000_000_000).toFixed(1)} GB/s`
    if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} MB/s`
    if (bps >= 1_000) return `${(bps / 1_000).toFixed(0)} KB/s`
    return `${Math.round(bps)} B/s`
}

export default face(
    'System Pulse',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        cpuLoadSensor: sensor('CPU Load Sensor', 'cpu_load', { group: 'Sensors' }),
        cpuTempSensor: sensor('CPU Temp Sensor', 'cpu_temp', { group: 'Sensors' }),
        gpuTempSensor: sensor('GPU Temp Sensor', 'gpu_temp', { group: 'Sensors' }),
        heroFont: font('Hero Font', 'Rajdhani', {
            families: [...DISPLAY_FONT_FAMILIES],
            group: 'Typography',
            weights: [600],
        }),
        hourFormat: combo('Clock Format', ['24h', '12h'], { group: 'Clock' }),
        ramSensor: sensor('RAM Sensor', 'ram_used', { group: 'Sensors' }),
        secondaryAccent: color('Secondary', palette.electricPurple, { group: 'Style' }),
        showClock: toggle('Show Clock', true, { group: 'Elements' }),
        showNet: toggle('Show Network', true, { group: 'Elements' }),
        showRig: toggle('Show Rig Colors', true, { group: 'Elements' }),
        transparentBackground: transparentBackgroundControl(),
        uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography', weights: [600] }),
    },
    {
        author: 'Hypercolor',
        description:
            'The everything dashboard: clock, animated metric cards, live network throughput, and your rig colors in one face.',
        designBasis: { height: 480, width: 480 },
        lighting: true,
        net: true,
        presets: [
            {
                controls: { accent: palette.neonCyan, secondaryAccent: palette.electricPurple },
                description: 'The full SilkCircuit dashboard.',
                name: 'Pulse',
            },
            {
                controls: {
                    accent: palette.coral,
                    secondaryAccent: '#ffb8dd',
                    showNet: false,
                    showRig: false,
                },
                description: 'Clock and metric cards only, coral chrome.',
                name: 'Essentials',
            },
            {
                controls: {
                    accent: '#50fa7b',
                    secondaryAccent: palette.neonCyan,
                    showClock: false,
                },
                description: 'Metrics and throughput, no clock — pure telemetry.',
                name: 'Ops Deck',
            },
        ],
        variants: {
            wide: (ctx: FaceContext) => buildSystemPulse(ctx, true),
        },
    },
    (ctx) => buildSystemPulse(ctx, false),
)

interface NetPanel {
    update(rxBps: number, txBps: number, iface: string): void
    setAccent(color: string): void
}

function createNetPanel(parent: HTMLElement, accent: string, width: number, height: number): NetPanel {
    const root = document.createElement('div')
    root.className = 'hc-pulse__net'
    root.style.height = `${height}px`
    parent.appendChild(root)

    const canvas = document.createElement('canvas')
    canvas.className = 'hc-pulse__net-canvas'
    canvas.width = width
    canvas.height = height
    root.appendChild(canvas)
    const chartCtx = canvas.getContext('2d')

    const head = document.createElement('div')
    head.className = 'hc-pulse__net-head'
    head.innerHTML = `
        <span class="hc-pulse__net-label">NET</span>
        <span class="hc-pulse__net-rates">
            <span class="hc-pulse__net-down">&#8595; --</span>
            <span class="hc-pulse__net-up">&#8593; --</span>
        </span>`
    root.appendChild(head)

    const labelEl = head.querySelector<HTMLSpanElement>('.hc-pulse__net-label')
    const downEl = head.querySelector<HTMLSpanElement>('.hc-pulse__net-down')
    const upEl = head.querySelector<HTMLSpanElement>('.hc-pulse__net-up')

    const rxHistory = new ValueHistory(NET_HISTORY)
    const txHistory = new ValueHistory(NET_HISTORY)
    let scale = NET_SCALE_FLOOR_BPS
    let lineColor = accent

    return {
        setAccent(color) {
            lineColor = color
        },
        update(rxBps, txBps, iface) {
            rxHistory.push(rxBps)
            txHistory.push(txBps)
            const observed = Math.max(...rxHistory.values(), ...txHistory.values(), NET_SCALE_FLOOR_BPS)
            scale = scale + (observed - scale) * 0.2

            if (downEl) downEl.innerHTML = `&#8595; ${formatBps(rxBps)}`
            if (upEl) upEl.innerHTML = `&#8593; ${formatBps(txBps)}`
            if (labelEl) labelEl.textContent = iface ? `NET · ${iface.toUpperCase()}` : 'NET'

            if (!chartCtx) return
            chartCtx.clearRect(0, 0, canvas.width, canvas.height)
            sparkline(chartCtx, {
                color: lineColor,
                fillOpacity: 0.18,
                height: canvas.height - 6,
                range: [0, scale],
                values: rxHistory.values(),
                width: canvas.width,
                x: 0,
                y: 4,
            })
            sparkline(chartCtx, {
                color: withAlpha(lineColor, 0.45),
                fill: false,
                height: canvas.height - 6,
                range: [0, scale],
                values: txHistory.values(),
                width: canvas.width,
                x: 0,
                y: 4,
            })
        },
    }
}

function buildSystemPulse(ctx: FaceContext, wide: boolean) {
    ensureFaceStyles(STYLE_ID, STYLES)
    const root = createFaceRoot(ctx, 'hc-pulse')
    root.classList.toggle('hc-pulse--wide', wide)

    const safe = ctx.display.safeArea
    const contentWidth = wide ? ctx.width * 0.94 : safe.width
    const heroScale = wide ? ctx.height / 480 : Math.min(safe.width, safe.height) / 339

    const stack = document.createElement('div')
    stack.className = 'hc-pulse__stack'
    stack.style.width = `${contentWidth}px`
    root.appendChild(stack)

    const clockEl = document.createElement('div')
    clockEl.className = 'hc-pulse__clock'
    clockEl.innerHTML = `
        <div class="hc-pulse__time">00:00</div>
        <div class="hc-pulse__date">MON JAN 1</div>`
    stack.appendChild(clockEl)
    const timeEl = clockEl.querySelector<HTMLDivElement>('.hc-pulse__time')
    const dateEl = clockEl.querySelector<HTMLDivElement>('.hc-pulse__date')
    if (timeEl) timeEl.style.fontSize = `${Math.round((wide ? 96 : 64) * heroScale)}px`
    if (dateEl) dateEl.style.fontSize = `${Math.round(11 * Math.max(heroScale, 0.9))}px`

    const cardsEl = document.createElement('div')
    cardsEl.className = 'hc-pulse__cards'

    const sideEl = wide ? document.createElement('div') : stack
    if (wide) sideEl.className = 'hc-pulse__side'

    stack.appendChild(cardsEl)
    if (wide) stack.appendChild(sideEl)

    const accent = palette.neonCyan
    const cardSpecs = [
        { key: 'cpuTempSensor', label: 'CPU' },
        { key: 'gpuTempSensor', label: 'GPU' },
        { key: 'ramSensor', label: 'RAM' },
    ] as const
    const cards: { key: string; card: MetricCard }[] = cardSpecs.map((spec) => ({
        card: createMetricCard(cardsEl, {
            accent,
            label: spec.label,
            sparkline: true,
        }),
        key: spec.key,
    }))

    const netPanel = createNetPanel(
        sideEl,
        accent,
        wide ? contentWidth * 0.3 : contentWidth,
        Math.round((wide ? 64 : 56) * Math.max(heroScale, 0.8)),
    )
    const netEl = sideEl.querySelector<HTMLDivElement>('.hc-pulse__net') ?? sideEl

    const rigEl = document.createElement('div')
    rigEl.className = 'hc-pulse__rig'
    rigEl.innerHTML = `
        <span class="hc-pulse__rig-scene">RIG</span>
        <span class="hc-pulse__rig-strip"></span>`
    sideEl.appendChild(rigEl)
    const rigSceneEl = rigEl.querySelector<HTMLSpanElement>('.hc-pulse__rig-scene')
    const rigStripEl = rigEl.querySelector<HTMLSpanElement>('.hc-pulse__rig-strip')
    if (rigSceneEl) rigSceneEl.style.fontSize = `${Math.round(10 * Math.max(heroScale, 0.9))}px`

    let lastTime = Number.NaN
    let lastRigKey = ''
    let lastAccent = ''
    const drifters = makeDrifters(wide ? 28 : 18)

    return (
        time: number,
        controls: Record<string, unknown>,
        sensors: import('@hypercolor/sdk').SensorAccessor,
        _audio: import('@hypercolor/sdk').AudioAccessor,
        data: FaceDataSources,
    ) => {
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time
        const accentColor = controls.accent as string
        const secondary = controls.secondaryAccent as string
        const ink = resolveFaceInk(accentColor)

        root.style.setProperty('--accent', accentColor)
        root.style.setProperty('--secondary', secondary)
        root.style.setProperty('--hero-ink', ink.hero)
        root.style.setProperty('--ui-ink', ink.ui)
        root.style.setProperty('--dim-ink', ink.dim)
        root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
        root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
        if (accentColor !== lastAccent) {
            lastAccent = accentColor
            for (const { card } of cards) card.setAccent(accentColor)
            netPanel.setAccent(accentColor)
        }

        const now = new Date()
        let hours = now.getHours()
        if (controls.hourFormat === '12h') hours = hours % 12 || 12
        if (timeEl) {
            timeEl.textContent = `${hours}:${now.getMinutes().toString().padStart(2, '0')}`
        }
        if (dateEl) {
            dateEl.textContent = now
                .toLocaleDateString('en-US', { day: 'numeric', month: 'short', weekday: 'short' })
                .toUpperCase()
        }
        clockEl.classList.toggle('hc-pulse__hidden', controls.showClock !== true)

        for (const { key, card } of cards) {
            const label = controls[key] as string
            card.update({
                dt,
                normalized: clamp01(sensors.normalized(label)),
                text: sensors.formatted(label),
            })
        }

        const showNet = controls.showNet === true
        netEl.classList.toggle('hc-pulse__hidden', !showNet)
        if (showNet) {
            const net = data.net.state()
            netPanel.update(net.rxBps, net.txBps, net.iface)
        }

        const showRig = controls.showRig === true
        rigEl.classList.toggle('hc-pulse__hidden', !showRig)
        if (showRig && rigStripEl && rigSceneEl) {
            const lighting = data.lighting.state()
            const rigKey = `${lighting.sceneName ?? ''}|${lighting.dominantColors.join(',')}`
            if (rigKey !== lastRigKey) {
                lastRigKey = rigKey
                rigSceneEl.textContent = (lighting.sceneName ?? 'RIG').toUpperCase()
                if (lighting.dominantColors.length > 1) {
                    rigStripEl.style.background = `linear-gradient(90deg, ${lighting.dominantColors.join(', ')})`
                } else if (lighting.dominantColors.length === 1) {
                    rigStripEl.style.background = lighting.dominantColors[0] ?? ''
                } else {
                    rigStripEl.style.background = 'rgba(255,255,255,0.06)'
                }
            }
        }

        ctx.ctx.clearRect(0, 0, ctx.width, ctx.height)
        if (atmosphereVisible(controls)) {
            drawNebulaField(ctx.ctx, ctx.width, ctx.height, time, accentColor, secondary, 1.0)
            drawRisingMotes(ctx.ctx, ctx.width, ctx.height, time, drifters, accentColor, 0.6, 0.4)
        }
    }
}
