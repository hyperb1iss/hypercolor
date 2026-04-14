import { color, combo, face, font, lerpColor, num, palette, sensor, toggle } from '@hypercolor/sdk'

import {
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    mixFaceAccent,
    resolveFaceInk,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-silkcircuit-hud'

function requireElement<T extends Element>(root: ParentNode, selector: string): T {
    const element = root.querySelector<T>(selector)
    if (!element) {
        throw new Error(`Missing required SilkCircuit HUD element: ${selector}`)
    }
    return element
}

function requireParentElement(element: Element, selector: string): HTMLElement {
    const parent = element.parentElement
    if (!parent) {
        throw new Error(`Missing required parent element for: ${selector}`)
    }
    return parent
}

const STYLES = `
.hc-silk-hud {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.coral};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --clock-size: 84;
    --metric-size: 56;
    --detail-size: 11;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
    display: grid;
    place-items: center;
}

.hc-silk-hud__stack {
    display: grid;
    gap: 18px;
    justify-items: center;
    align-items: center;
    text-align: center;
    width: min(78%, 420px);
}

.hc-silk-hud__clock {
    display: grid;
    gap: 10px;
    justify-items: center;
    align-items: center;
}

.hc-silk-hud__time {
    display: inline-flex;
    align-items: baseline;
    justify-content: center;
    gap: 8px;
    font-family: var(--hero-font);
    font-size: calc(var(--clock-size) * 1px);
    font-weight: 600;
    line-height: 0.86;
    letter-spacing: 0.015em;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    text-shadow:
        0 0 18px color-mix(in srgb, var(--accent) 12%, transparent),
        0 8px 24px rgba(0,0,0,0.24);
}

.hc-silk-hud__slot {
    display: inline-grid;
    grid-auto-flow: column;
    justify-content: center;
    grid-template-columns: repeat(2, 0.66ch);
}

.hc-silk-hud__digit {
    display: inline-flex;
    width: 0.66ch;
    justify-content: center;
}

.hc-silk-hud__digit--blank {
    opacity: 0;
}

.hc-silk-hud__separator {
    color: var(--dim-ink);
    transform: translateY(-2px);
}

.hc-silk-hud__date {
    font-family: var(--ui-font);
    font-size: calc((var(--detail-size) + 1) * 1px);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-silk-hud__metrics {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 20px;
    width: 100%;
    justify-items: center;
    align-items: center;
}

.hc-silk-hud__metric {
    display: grid;
    gap: 6px;
    justify-items: center;
    align-items: center;
    text-align: center;
    background: transparent;
    border: none;
    padding: 0;
}

.hc-silk-hud__metric-label {
    font-family: var(--ui-font);
    font-size: calc(var(--detail-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-silk-hud__metric-value {
    font-family: var(--hero-font);
    font-size: calc(var(--metric-size) * 1px);
    font-weight: 600;
    line-height: 0.9;
    text-align: center;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
}

.hc-silk-hud__bars {
    display: grid;
    gap: 10px;
    width: 100%;
}

.hc-silk-hud__bar {
    display: grid;
    gap: 6px;
    padding: 0;
    background: transparent;
    border: none;
    width: 100%;
}

.hc-silk-hud__bar-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-family: var(--ui-font);
    font-size: calc(var(--detail-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--ui-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
}

.hc-silk-hud__bar-rail {
    position: relative;
    height: 6px;
    border-radius: 999px;
    overflow: hidden;
    background: rgba(255,255,255,0.06);
}

.hc-silk-hud__bar-fill {
    position: absolute;
    inset: 0 auto 0 0;
    width: calc(var(--fill, 0) * 100%);
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), var(--secondary));
}

.hc-silk-hud__hidden {
    display: none !important;
}
`

function setHudDigit(slot: HTMLSpanElement, value: string | null): void {
    slot.textContent = value ?? '0'
    slot.classList.toggle('hc-silk-hud__digit--blank', value == null)
}

export default face(
    'SilkCircuit HUD',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        clockSize: num('Clock Size', [48, 128], 84, { group: 'Typography' }),
        cpuLoadSensor: sensor('CPU Load Sensor', 'cpu_load', { group: 'Sensors' }),
        cpuTempSensor: sensor('CPU Temp Sensor', 'cpu_temp', { group: 'Sensors' }),
        detailSize: num('Detail Size', [9, 20], 11, { group: 'Typography' }),
        gpuTempSensor: sensor('GPU Temp Sensor', 'gpu_temp', { group: 'Sensors' }),
        heroFont: font('Hero Font', 'Rajdhani', { families: [...DISPLAY_FONT_FAMILIES], group: 'Typography' }),
        hourFormat: combo('Clock Format', ['24h', '12h'], { group: 'Clock' }),
        metricSize: num('Metric Size', [28, 92], 56, { group: 'Typography' }),
        ramSensor: sensor('RAM Sensor', 'ram_used', { group: 'Sensors' }),
        secondaryAccent: color('Secondary', palette.coral, { group: 'Style' }),
        showBarLabels: toggle('Show Bar Labels', true, { group: 'Elements' }),
        showBars: toggle('Show Bars', true, { group: 'Elements' }),
        showClock: toggle('Show Clock', true, { group: 'Elements' }),
        showDate: toggle('Show Date', true, { group: 'Elements' }),
        showMetricLabels: toggle('Show Metric Labels', true, { group: 'Elements' }),
        showMetrics: toggle('Show Metrics', true, { group: 'Elements' }),
        uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography' }),
    },
    {
        author: 'Hypercolor',
        description: 'A clean command-center face. Every element is independently toggleable.',
        designBasis: { height: 480, width: 480 },
        presets: [
            {
                controls: {
                    accent: palette.neonCyan,
                    heroFont: 'Rajdhani',
                    secondaryAccent: palette.coral,
                    uiFont: 'Inter',
                },
                description: 'The classic SilkCircuit cyan/coral command deck.',
                name: 'Signature HUD',
            },
            {
                controls: {
                    accent: '#ffb347',
                    heroFont: 'Roboto Condensed',
                    secondaryAccent: '#ff6b6b',
                    uiFont: 'Inter',
                },
                description: 'Warm amber chrome and bold numerals.',
                name: 'Forge Deck',
            },
            {
                controls: {
                    accent: '#9ae7ff',
                    heroFont: 'Exo 2',
                    secondaryAccent: '#c8d5ff',
                    uiFont: 'Inter',
                },
                description: 'Cool blue minimal HUD with airy type.',
                name: 'Arctic Rail',
            },
            {
                controls: {
                    accent: palette.coral,
                    heroFont: 'Exo 2',
                    secondaryAccent: '#ffb8dd',
                    uiFont: 'DM Sans',
                },
                description: 'Coral-forward variant with soft contrast.',
                name: 'Rose Protocol',
            },
            {
                controls: {
                    accent: palette.electricYellow,
                    heroFont: 'Space Mono',
                    secondaryAccent: '#ffa166',
                    uiFont: 'JetBrains Mono',
                },
                description: 'Sharper monospaced telemetry.',
                name: 'Mono Grid',
            },
            {
                controls: {
                    accent: palette.neonCyan,
                    heroFont: 'Rajdhani',
                    secondaryAccent: palette.electricPurple,
                    showBars: false,
                    showMetrics: false,
                    uiFont: 'Inter',
                },
                description: 'Just the clock, centered and clean.',
                name: 'Clock Only',
            },
            {
                controls: {
                    accent: '#8fe8ff',
                    heroFont: 'Orbitron',
                    secondaryAccent: '#7fa2ff',
                    uiFont: 'DM Sans',
                },
                description: 'Blue-cyan bridge with calm utility text.',
                name: 'Signal Bridge',
            },
            {
                controls: {
                    accent: '#ffb25f',
                    heroFont: 'Roboto Condensed',
                    secondaryAccent: '#ff7d8e',
                    showClock: false,
                    showDate: false,
                    uiFont: 'Space Grotesk',
                },
                description: 'Just the metric tiles with bars.',
                name: 'Metrics Only',
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-silk-hud')
        root.innerHTML = `
            <div class="hc-silk-hud__stack">
                <div class="hc-silk-hud__clock">
                    <div class="hc-silk-hud__time">
                        <span class="hc-silk-hud__slot hc-silk-hud__slot--hours">
                            <span class="hc-silk-hud__digit hc-silk-hud__hours-tens">0</span>
                            <span class="hc-silk-hud__digit hc-silk-hud__hours-ones">0</span>
                        </span>
                        <span class="hc-silk-hud__separator">:</span>
                        <span class="hc-silk-hud__slot hc-silk-hud__slot--minutes">
                            <span class="hc-silk-hud__digit hc-silk-hud__minutes-tens">0</span>
                            <span class="hc-silk-hud__digit hc-silk-hud__minutes-ones">0</span>
                        </span>
                    </div>
                    <div class="hc-silk-hud__date">MON MAY 15</div>
                </div>
                <div class="hc-silk-hud__metrics">
                    <div class="hc-silk-hud__metric hc-silk-hud__cpu">
                        <div class="hc-silk-hud__metric-label">CPU TEMP</div>
                        <div class="hc-silk-hud__metric-value">--</div>
                    </div>
                    <div class="hc-silk-hud__metric hc-silk-hud__gpu">
                        <div class="hc-silk-hud__metric-label">GPU TEMP</div>
                        <div class="hc-silk-hud__metric-value">--</div>
                    </div>
                </div>
                <div class="hc-silk-hud__bars">
                    <div class="hc-silk-hud__bar">
                        <div class="hc-silk-hud__bar-head"><span class="hc-silk-hud__load-label">CPU LOAD</span><span class="hc-silk-hud__load-value">--</span></div>
                        <div class="hc-silk-hud__bar-rail"><div class="hc-silk-hud__bar-fill hc-silk-hud__load-fill"></div></div>
                    </div>
                    <div class="hc-silk-hud__bar">
                        <div class="hc-silk-hud__bar-head"><span class="hc-silk-hud__ram-label">RAM</span><span class="hc-silk-hud__ram-value">--</span></div>
                        <div class="hc-silk-hud__bar-rail"><div class="hc-silk-hud__bar-fill hc-silk-hud__ram-fill"></div></div>
                    </div>
                </div>
            </div>
        `

        const clockEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__clock')
        const hoursTensEl = requireElement<HTMLSpanElement>(root, '.hc-silk-hud__hours-tens')
        const hoursOnesEl = requireElement<HTMLSpanElement>(root, '.hc-silk-hud__hours-ones')
        const minutesTensEl = requireElement<HTMLSpanElement>(root, '.hc-silk-hud__minutes-tens')
        const minutesOnesEl = requireElement<HTMLSpanElement>(root, '.hc-silk-hud__minutes-ones')
        const dateEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__date')
        const metricsEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__metrics')
        const cpuLabelEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__cpu .hc-silk-hud__metric-label')
        const gpuLabelEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__gpu .hc-silk-hud__metric-label')
        const cpuValueEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__cpu .hc-silk-hud__metric-value')
        const gpuValueEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__gpu .hc-silk-hud__metric-value')
        const loadLabelEl = requireElement<HTMLSpanElement>(root, '.hc-silk-hud__load-label')
        const loadValueEl = requireElement<HTMLSpanElement>(root, '.hc-silk-hud__load-value')
        const ramLabelEl = requireElement<HTMLSpanElement>(root, '.hc-silk-hud__ram-label')
        const ramValueEl = requireElement<HTMLSpanElement>(root, '.hc-silk-hud__ram-value')
        const loadFillEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__load-fill')
        const ramFillEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__ram-fill')
        const barsEl = requireElement<HTMLDivElement>(root, '.hc-silk-hud__bars')
        const loadHeadEl = requireParentElement(loadLabelEl, '.hc-silk-hud__load-label')
        const ramHeadEl = requireParentElement(ramLabelEl, '.hc-silk-hud__ram-label')

        return (_time, controls, sensors) => {
            const accent = lerpColor(controls.accent as string, palette.fg.primary, 0.05)
            const secondary = mixFaceAccent(controls.secondaryAccent as string, accent, 0.14)
            const ink = resolveFaceInk(accent)

            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--hero-ink', ink.hero)
            root.style.setProperty('--ui-ink', ink.ui)
            root.style.setProperty('--dim-ink', ink.dim)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--clock-size', `${controls.clockSize as number}`)
            root.style.setProperty('--metric-size', `${controls.metricSize as number}`)
            root.style.setProperty('--detail-size', `${controls.detailSize as number}`)

            const now = new Date()
            let hours = now.getHours()
            const minutes = now.getMinutes()
            if (controls.hourFormat === '12h') hours = hours % 12 || 12
            const hourText = hours.toString()
            const minuteText = minutes.toString().padStart(2, '0')
            setHudDigit(hoursTensEl, hourText.length > 1 ? (hourText[0] ?? null) : null)
            setHudDigit(hoursOnesEl, hourText[hourText.length - 1] ?? '0')
            setHudDigit(minutesTensEl, minuteText[0] ?? '0')
            setHudDigit(minutesOnesEl, minuteText[1] ?? '0')
            dateEl.textContent = now
                .toLocaleDateString('en-US', { day: 'numeric', month: 'short', weekday: 'short' })
                .toUpperCase()

            const cpuLoad = sensors.normalized(controls.cpuLoadSensor as string)
            const ram = sensors.normalized(controls.ramSensor as string)
            cpuValueEl.textContent = sensors.formatted(controls.cpuTempSensor as string)
            gpuValueEl.textContent = sensors.formatted(controls.gpuTempSensor as string)
            loadValueEl.textContent = sensors.formatted(controls.cpuLoadSensor as string)
            ramValueEl.textContent = sensors.formatted(controls.ramSensor as string)
            loadFillEl.style.setProperty('--fill', Math.max(0, Math.min(1, cpuLoad)).toFixed(4))
            ramFillEl.style.setProperty('--fill', Math.max(0, Math.min(1, ram)).toFixed(4))

            const showClock = controls.showClock as boolean
            const showDate = controls.showDate as boolean
            const showMetrics = controls.showMetrics as boolean
            const showMetricLabels = controls.showMetricLabels as boolean
            const showBars = controls.showBars as boolean
            const showBarLabels = controls.showBarLabels as boolean

            clockEl.classList.toggle('hc-silk-hud__hidden', !showClock)
            dateEl.classList.toggle('hc-silk-hud__hidden', !showDate)
            metricsEl.classList.toggle('hc-silk-hud__hidden', !showMetrics)
            cpuLabelEl.classList.toggle('hc-silk-hud__hidden', !showMetricLabels)
            gpuLabelEl.classList.toggle('hc-silk-hud__hidden', !showMetricLabels)
            barsEl.classList.toggle('hc-silk-hud__hidden', !showBars)
            loadHeadEl.classList.toggle('hc-silk-hud__hidden', !showBarLabels)
            ramHeadEl.classList.toggle('hc-silk-hud__hidden', !showBarLabels)

            const c = ctx.ctx
            c.clearRect(0, 0, ctx.width, ctx.height)
        }
    },
)
