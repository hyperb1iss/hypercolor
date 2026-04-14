import {
    color,
    combo,
    face,
    font,
    lerpColor,
    num,
    palette,
    sensor,
    toggle,
} from '@hypercolor/sdk'

import {
    DISPLAY_FONT_FAMILIES,
    UI_FONT_FAMILIES,
    createFaceRoot,
    ensureFaceStyles,
    mixFaceAccent,
    resolveFaceInk,
} from '../shared/dom'

const STYLE_ID = 'hc-face-silkcircuit-hud'

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
        cpuTempSensor: sensor('CPU Temp Sensor', 'cpu_temp', { group: 'Sensors' }),
        gpuTempSensor: sensor('GPU Temp Sensor', 'gpu_temp', { group: 'Sensors' }),
        cpuLoadSensor: sensor('CPU Load Sensor', 'cpu_load', { group: 'Sensors' }),
        ramSensor: sensor('RAM Sensor', 'ram_used', { group: 'Sensors' }),
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        secondaryAccent: color('Secondary', palette.coral, { group: 'Style' }),
        heroFont: font('Hero Font', 'Rajdhani', { group: 'Typography', families: [...DISPLAY_FONT_FAMILIES] }),
        uiFont: font('UI Font', 'Inter', { group: 'Typography', families: [...UI_FONT_FAMILIES] }),
        clockSize: num('Clock Size', [48, 128], 84, { group: 'Typography' }),
        metricSize: num('Metric Size', [28, 92], 56, { group: 'Typography' }),
        detailSize: num('Detail Size', [9, 20], 11, { group: 'Typography' }),
        hourFormat: combo('Clock Format', ['24h', '12h'], { group: 'Clock' }),
        showClock: toggle('Show Clock', true, { group: 'Elements' }),
        showDate: toggle('Show Date', true, { group: 'Elements' }),
        showMetrics: toggle('Show Metrics', true, { group: 'Elements' }),
        showMetricLabels: toggle('Show Metric Labels', true, { group: 'Elements' }),
        showBars: toggle('Show Bars', true, { group: 'Elements' }),
        showBarLabels: toggle('Show Bar Labels', true, { group: 'Elements' }),
    },
    {
        description: 'A clean command-center face. Every element is independently toggleable.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'Signature HUD',
                description: 'The classic SilkCircuit cyan/coral command deck.',
                controls: {
                    accent: palette.neonCyan,
                    secondaryAccent: palette.coral,
                    heroFont: 'Rajdhani',
                    uiFont: 'Inter',
                },
            },
            {
                name: 'Forge Deck',
                description: 'Warm amber chrome and bold numerals.',
                controls: {
                    accent: '#ffb347',
                    secondaryAccent: '#ff6b6b',
                    heroFont: 'Roboto Condensed',
                    uiFont: 'Inter',
                },
            },
            {
                name: 'Arctic Rail',
                description: 'Cool blue minimal HUD with airy type.',
                controls: {
                    accent: '#9ae7ff',
                    secondaryAccent: '#c8d5ff',
                    heroFont: 'Exo 2',
                    uiFont: 'Inter',
                },
            },
            {
                name: 'Rose Protocol',
                description: 'Coral-forward variant with soft contrast.',
                controls: {
                    accent: palette.coral,
                    secondaryAccent: '#ffb8dd',
                    heroFont: 'Exo 2',
                    uiFont: 'DM Sans',
                },
            },
            {
                name: 'Mono Grid',
                description: 'Sharper monospaced telemetry.',
                controls: {
                    accent: palette.electricYellow,
                    secondaryAccent: '#ffa166',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                },
            },
            {
                name: 'Clock Only',
                description: 'Just the clock, centered and clean.',
                controls: {
                    accent: palette.neonCyan,
                    secondaryAccent: palette.electricPurple,
                    heroFont: 'Rajdhani',
                    uiFont: 'Inter',
                    showMetrics: false,
                    showBars: false,
                },
            },
            {
                name: 'Signal Bridge',
                description: 'Blue-cyan bridge with calm utility text.',
                controls: {
                    accent: '#8fe8ff',
                    secondaryAccent: '#7fa2ff',
                    heroFont: 'Orbitron',
                    uiFont: 'DM Sans',
                },
            },
            {
                name: 'Metrics Only',
                description: 'Just the metric tiles with bars.',
                controls: {
                    accent: '#ffb25f',
                    secondaryAccent: '#ff7d8e',
                    heroFont: 'Roboto Condensed',
                    uiFont: 'Space Grotesk',
                    showClock: false,
                    showDate: false,
                },
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

        const clockEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__clock')!
        const hoursTensEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__hours-tens')!
        const hoursOnesEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__hours-ones')!
        const minutesTensEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__minutes-tens')!
        const minutesOnesEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__minutes-ones')!
        const dateEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__date')!
        const metricsEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__metrics')!
        const cpuLabelEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__cpu .hc-silk-hud__metric-label')!
        const gpuLabelEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__gpu .hc-silk-hud__metric-label')!
        const cpuValueEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__cpu .hc-silk-hud__metric-value')!
        const gpuValueEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__gpu .hc-silk-hud__metric-value')!
        const loadLabelEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__load-label')!
        const loadValueEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__load-value')!
        const ramLabelEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__ram-label')!
        const ramValueEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__ram-value')!
        const loadFillEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__load-fill')!
        const ramFillEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__ram-fill')!
        const barsEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__bars')!
        const loadHeadEl = loadLabelEl.parentElement!
        const ramHeadEl = ramLabelEl.parentElement!

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
            setHudDigit(hoursTensEl, hourText.length > 1 ? hourText[0] ?? null : null)
            setHudDigit(hoursOnesEl, hourText[hourText.length - 1] ?? '0')
            setHudDigit(minutesTensEl, minuteText[0] ?? '0')
            setHudDigit(minutesOnesEl, minuteText[1] ?? '0')
            dateEl.textContent = now
                .toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric' })
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
