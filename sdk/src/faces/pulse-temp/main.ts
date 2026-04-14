import {
    arcGauge,
    color,
    colorByValue,
    combo,
    face,
    font,
    num,
    palette,
    sensor,
    toggle,
    ValueHistory,
    withAlpha,
} from '@hypercolor/sdk'

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

const STYLE_ID = 'hc-face-pulse-temp'
const FACE_SCHEMES = {
    load: ['#50fa7b', '#00d4ff', '#ff5ca8'] as const,
    memory: ['#77ecff', '#8f70ff'] as const,
    temperature: ['#7ce9ff', '#ffb35f', '#ff6b7a'] as const,
}

const STYLES = `
.hc-pulse-temp {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.coral};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --hero-size: 132;
    --detail-size: 12;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
}

.hc-pulse-temp__value {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    display: inline-flex;
    align-items: baseline;
    justify-content: center;
    gap: 8px;
    line-height: 1;
    white-space: nowrap;
}

.hc-pulse-temp__number {
    font-family: var(--hero-font);
    font-size: calc(var(--hero-size) * 1px);
    font-weight: 600;
    line-height: 1;
    letter-spacing: 0.015em;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    text-shadow:
        0 0 20px color-mix(in srgb, var(--accent) 12%, transparent),
        0 10px 28px rgba(0, 0, 0, 0.28);
}

.hc-pulse-temp__unit {
    font-family: var(--ui-font);
    font-size: 32px;
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-pulse-temp__label {
    position: absolute;
    top: calc(50% + 74px);
    left: 50%;
    transform: translateX(-50%);
    font-family: var(--ui-font);
    font-size: calc(var(--detail-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--ui-ink);
    white-space: nowrap;
}

.hc-pulse-temp__details {
    position: absolute;
    top: calc(50% + 100px);
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 18px;
    flex-wrap: nowrap;
    font-family: var(--ui-font);
    font-size: calc((var(--detail-size) - 1) * 1px);
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--dim-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    white-space: nowrap;
}

.hc-pulse-temp__detail--primary {
    color: var(--ui-ink);
}

.hc-pulse-temp__hidden {
    display: none !important;
}
`

export default face(
    'Pulse Temp',
    {
        colorScheme: combo('Color Scheme', ['Temperature', 'Load', 'Memory', 'Custom'], { group: 'Style' }),
        customColor: color('Custom Color', palette.neonCyan, { group: 'Style' }),
        detailSize: num('Detail Size', [9, 24], 12, { group: 'Typography' }),
        glowIntensity: num('Glow', [0, 100], 54, { group: 'Style' }),
        heroFont: font('Hero Font', 'Rajdhani', { families: [...DISPLAY_FONT_FAMILIES], group: 'Typography' }),
        heroSize: num('Hero Size', [72, 164], 132, { group: 'Typography' }),
        meterStyle: combo('Meter Style', ['Halo', 'Vector', 'Scope'], { group: 'Layout' }),
        showArc: toggle('Show Arc', true, { group: 'Elements' }),
        showLabel: toggle('Show Label', true, { group: 'Elements' }),
        showNumber: toggle('Show Number', true, { group: 'Elements' }),
        showPeak: toggle('Show Peak', false, { group: 'Elements' }),
        showTrend: toggle('Show Trend', false, { group: 'Elements' }),
        showUnit: toggle('Show Unit', true, { group: 'Elements' }),
        targetSensor: sensor('Sensor', 'cpu_temp', { group: 'Data' }),
        uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography' }),
    },
    {
        author: 'Hypercolor',
        description: 'A centered single-sensor readout. Every element is independently toggleable.',
        designBasis: { height: 480, width: 480 },
        presets: [
            {
                controls: {
                    colorScheme: 'Temperature',
                    glowIntensity: 60,
                    heroFont: 'Rajdhani',
                    meterStyle: 'Halo',
                    targetSensor: 'cpu_temp',
                    uiFont: 'Inter',
                },
                description: 'Cyan-to-hot thermal watch with clear tech numerals.',
                name: 'CPU Siren',
            },
            {
                controls: {
                    colorScheme: 'Temperature',
                    glowIntensity: 54,
                    heroFont: 'Roboto Condensed',
                    meterStyle: 'Vector',
                    targetSensor: 'gpu_temp',
                    uiFont: 'Inter',
                },
                description: 'Warm overclock mood with bold condensed numerals.',
                name: 'GPU Ember',
            },
            {
                controls: {
                    colorScheme: 'Load',
                    glowIntensity: 58,
                    heroFont: 'Exo 2',
                    meterStyle: 'Scope',
                    targetSensor: 'cpu_load',
                    uiFont: 'DM Sans',
                },
                description: 'Green-magenta load readout with compact secondary detail.',
                name: 'Load Bloom',
            },
            {
                controls: {
                    colorScheme: 'Memory',
                    glowIntensity: 42,
                    heroFont: 'Rajdhani',
                    meterStyle: 'Halo',
                    targetSensor: 'ram_used',
                    uiFont: 'Inter',
                },
                description: 'Clean violet memory monitor.',
                name: 'Memory Core',
            },
            {
                controls: {
                    colorScheme: 'Custom',
                    customColor: palette.coral,
                    glowIntensity: 52,
                    heroFont: 'Exo 2',
                    meterStyle: 'Scope',
                    targetSensor: 'cpu_temp',
                    uiFont: 'Space Grotesk',
                },
                description: 'Custom coral readout with softer chrome.',
                name: 'Coral Signal',
            },
            {
                controls: {
                    colorScheme: 'Load',
                    glowIntensity: 34,
                    heroFont: 'Space Mono',
                    meterStyle: 'Vector',
                    targetSensor: 'gpu_load',
                    uiFont: 'JetBrains Mono',
                },
                description: 'Sharper monospaced numerals.',
                name: 'Mono Luxe',
            },
            {
                controls: {
                    colorScheme: 'Custom',
                    customColor: '#ffb35c',
                    glowIntensity: 50,
                    heroFont: 'Rajdhani',
                    meterStyle: 'Halo',
                    targetSensor: 'cpu_temp',
                    uiFont: 'DM Sans',
                },
                description: 'Warm gold thermal halo with clean sans meta.',
                name: 'Amber Core',
            },
            {
                controls: {
                    colorScheme: 'Temperature',
                    heroFont: 'Rajdhani',
                    showArc: false,
                    showLabel: false,
                    showPeak: false,
                    showTrend: false,
                    showUnit: false,
                    targetSensor: 'cpu_temp',
                    uiFont: 'Inter',
                },
                description: 'Just the number. No chrome, no arc, no meta.',
                name: 'Naked Digit',
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-pulse-temp')
        root.innerHTML = `
            <div class="hc-pulse-temp__value">
                <span class="hc-pulse-temp__number">--</span>
                <span class="hc-pulse-temp__unit">°C</span>
            </div>
            <div class="hc-pulse-temp__label">CPU TEMP</div>
            <div class="hc-pulse-temp__details">
                <span class="hc-pulse-temp__detail hc-pulse-temp__detail--primary hc-pulse-temp__trend">STEADY</span>
                <span class="hc-pulse-temp__detail hc-pulse-temp__peak">PEAK --</span>
            </div>
        `

        const valueEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__value')!
        const numberEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__number')!
        const unitEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__unit')!
        const labelEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__label')!
        const detailsEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__details')!
        const trendEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__trend')!
        const peakEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__peak')!

        let smoothValue = 0
        let lastHistoryPush = 0
        let peakReading = Number.NEGATIVE_INFINITY
        let peakDisplay = '--'
        let activeSensor = ''
        let history = new ValueHistory(48)

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (time, controls, sensors) => {
            const sensorLabel = controls.targetSensor as string
            const reading = sensors.read(sensorLabel)
            const normalized = sensors.normalized(sensorLabel)
            smoothValue += (normalized - smoothValue) * 0.08

            if (activeSensor !== sensorLabel) {
                activeSensor = sensorLabel
                peakReading = Number.NEGATIVE_INFINITY
                peakDisplay = '--'
                history = new ValueHistory(48)
            }

            const scheme = controls.colorScheme as string
            const baseAccent =
                scheme === 'Temperature'
                    ? colorByValue(smoothValue, FACE_SCHEMES.temperature)
                    : scheme === 'Load'
                      ? colorByValue(smoothValue, FACE_SCHEMES.load)
                      : scheme === 'Memory'
                        ? colorByValue(smoothValue, FACE_SCHEMES.memory)
                        : (controls.customColor as string)
            const secondary =
                scheme === 'Temperature'
                    ? mixFaceAccent(baseAccent, palette.coral, 0.34)
                    : scheme === 'Memory'
                      ? mixFaceAccent(baseAccent, palette.electricPurple, 0.48)
                      : mixFaceAccent(baseAccent)
            const ink = resolveFaceInk(baseAccent)
            const glow = clamp01((controls.glowIntensity as number) / 100)
            const meterStyle = (controls.meterStyle as string).toLowerCase()

            root.dataset.style = meterStyle
            root.style.setProperty('--accent', baseAccent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--hero-size', `${controls.heroSize as number}`)
            root.style.setProperty('--detail-size', `${controls.detailSize as number}`)
            root.style.setProperty('--hero-ink', ink.hero)
            root.style.setProperty('--ui-ink', ink.ui)
            root.style.setProperty('--dim-ink', ink.dim)

            const formatted = sensors.formatted(sensorLabel)
            const match = formatted.match(/^([\d.]+)\s*(.*)$/)
            numberEl.textContent = match?.[1] ?? formatted
            unitEl.textContent = match?.[2] || (reading?.unit ?? '')
            labelEl.textContent = humanizeSensorLabel(sensorLabel)

            if (time - lastHistoryPush > 0.12) {
                history.push(normalized)
                lastHistoryPush = time
            }
            if (reading?.value != null && reading.value >= peakReading) {
                peakReading = reading.value
                peakDisplay = formatted
            }
            peakEl.textContent = `PEAK ${peakDisplay}`
            const values = history.values()
            const trendDelta = values.length > 8 ? smoothValue - values[Math.max(0, values.length - 8)] : 0
            trendEl.textContent = trendDelta > 0.018 ? 'RISING' : trendDelta < -0.018 ? 'COOLING' : 'STEADY'

            const showNumber = controls.showNumber as boolean
            const showUnit = controls.showUnit as boolean
            const showLabel = controls.showLabel as boolean
            const showTrend = controls.showTrend as boolean
            const showPeak = controls.showPeak as boolean
            const showArc = controls.showArc as boolean

            numberEl.classList.toggle('hc-pulse-temp__hidden', !showNumber)
            unitEl.classList.toggle('hc-pulse-temp__hidden', !showUnit)
            valueEl.classList.toggle('hc-pulse-temp__hidden', !showNumber && !showUnit)
            labelEl.classList.toggle('hc-pulse-temp__hidden', !showLabel)
            trendEl.classList.toggle('hc-pulse-temp__hidden', !showTrend)
            peakEl.classList.toggle('hc-pulse-temp__hidden', !showPeak)
            detailsEl.classList.toggle('hc-pulse-temp__hidden', !showTrend && !showPeak)

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)
            if (!showArc) return

            c.save()
            c.globalAlpha = 0.92

            if (meterStyle === 'vector') {
                arcGauge(c, {
                    cx,
                    cy,
                    fillColor: [baseAccent, secondary],
                    glow: 0.18 + glow * 0.24,
                    radius: 134,
                    startAngle: Math.PI * 0.98,
                    sweep: Math.PI * 0.86,
                    thickness: 10,
                    trackColor: withAlpha(ink.ui, 0.1),
                    value: smoothValue,
                })
            } else if (meterStyle === 'scope') {
                arcGauge(c, {
                    cx,
                    cy,
                    fillColor: [baseAccent, secondary],
                    glow: 0.2 + glow * 0.28,
                    radius: 146,
                    startAngle: Math.PI * 0.74,
                    sweep: Math.PI * 1.12,
                    thickness: 12,
                    trackColor: withAlpha(ink.ui, 0.1),
                    value: smoothValue,
                })
            } else {
                arcGauge(c, {
                    cx,
                    cy,
                    fillColor: [baseAccent, secondary],
                    glow: 0.24 + glow * 0.32,
                    radius: 156,
                    startAngle: Math.PI * 0.72,
                    sweep: Math.PI * 1.42,
                    thickness: 16,
                    trackColor: withAlpha(ink.ui, 0.12),
                    value: smoothValue,
                })
            }

            c.restore()
        }
    },
)
