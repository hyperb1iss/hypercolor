import {
    ValueHistory,
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
    withAlpha,
} from '@hypercolor/sdk'

import {
    DISPLAY_FONT_FAMILIES,
    UI_FONT_FAMILIES,
    clamp01,
    createFaceRoot,
    ensureFaceStyles,
    humanizeSensorLabel,
    mixFaceAccent,
    resolveFaceInk,
} from '../shared/dom'

const STYLE_ID = 'hc-face-pulse-temp'
const FACE_SCHEMES = {
    temperature: ['#7ce9ff', '#ffb35f', '#ff6b7a'] as const,
    load: ['#50fa7b', '#00d4ff', '#ff5ca8'] as const,
    memory: ['#77ecff', '#8f70ff'] as const,
}

const STYLES = `
.hc-pulse-temp {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.coral};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
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
    font-size: 132px;
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
    font-size: 12px;
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
    font-size: 11px;
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
        targetSensor: sensor('Sensor', 'cpu_temp', { group: 'Data' }),
        colorScheme: combo('Color Scheme', ['Temperature', 'Load', 'Memory', 'Custom'], { group: 'Style' }),
        customColor: color('Custom Color', palette.neonCyan, { group: 'Style' }),
        meterStyle: combo('Meter Style', ['Halo', 'Vector', 'Scope'], { group: 'Layout' }),
        heroFont: font('Hero Font', 'Rajdhani', { group: 'Typography', families: [...DISPLAY_FONT_FAMILIES] }),
        uiFont: font('UI Font', 'Inter', { group: 'Typography', families: [...UI_FONT_FAMILIES] }),
        glowIntensity: num('Glow', [0, 100], 54, { group: 'Style' }),
        showNumber: toggle('Show Number', true, { group: 'Elements' }),
        showUnit: toggle('Show Unit', true, { group: 'Elements' }),
        showLabel: toggle('Show Label', true, { group: 'Elements' }),
        showTrend: toggle('Show Trend', false, { group: 'Elements' }),
        showPeak: toggle('Show Peak', false, { group: 'Elements' }),
        showArc: toggle('Show Arc', true, { group: 'Elements' }),
    },
    {
        description: 'A centered single-sensor readout. Every element is independently toggleable.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'CPU Siren',
                description: 'Cyan-to-hot thermal watch with clear tech numerals.',
                controls: {
                    targetSensor: 'cpu_temp',
                    colorScheme: 'Temperature',
                    meterStyle: 'Halo',
                    heroFont: 'Rajdhani',
                    uiFont: 'Inter',
                    glowIntensity: 60,
                },
            },
            {
                name: 'GPU Ember',
                description: 'Warm overclock mood with bold condensed numerals.',
                controls: {
                    targetSensor: 'gpu_temp',
                    colorScheme: 'Temperature',
                    meterStyle: 'Vector',
                    heroFont: 'Roboto Condensed',
                    uiFont: 'Inter',
                    glowIntensity: 54,
                },
            },
            {
                name: 'Load Bloom',
                description: 'Green-magenta load readout with compact secondary detail.',
                controls: {
                    targetSensor: 'cpu_load',
                    colorScheme: 'Load',
                    meterStyle: 'Scope',
                    heroFont: 'Exo 2',
                    uiFont: 'DM Sans',
                    glowIntensity: 58,
                },
            },
            {
                name: 'Memory Core',
                description: 'Clean violet memory monitor.',
                controls: {
                    targetSensor: 'ram_used',
                    colorScheme: 'Memory',
                    meterStyle: 'Halo',
                    heroFont: 'Rajdhani',
                    uiFont: 'Inter',
                    glowIntensity: 42,
                },
            },
            {
                name: 'Coral Signal',
                description: 'Custom coral readout with softer chrome.',
                controls: {
                    targetSensor: 'cpu_temp',
                    colorScheme: 'Custom',
                    meterStyle: 'Scope',
                    customColor: palette.coral,
                    heroFont: 'Exo 2',
                    uiFont: 'Space Grotesk',
                    glowIntensity: 52,
                },
            },
            {
                name: 'Mono Luxe',
                description: 'Sharper monospaced numerals.',
                controls: {
                    targetSensor: 'gpu_load',
                    colorScheme: 'Load',
                    meterStyle: 'Vector',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                    glowIntensity: 34,
                },
            },
            {
                name: 'Amber Core',
                description: 'Warm gold thermal halo with clean sans meta.',
                controls: {
                    targetSensor: 'cpu_temp',
                    colorScheme: 'Custom',
                    customColor: '#ffb35c',
                    meterStyle: 'Halo',
                    heroFont: 'Rajdhani',
                    uiFont: 'DM Sans',
                    glowIntensity: 50,
                },
            },
            {
                name: 'Naked Digit',
                description: 'Just the number. No chrome, no arc, no meta.',
                controls: {
                    targetSensor: 'cpu_temp',
                    colorScheme: 'Temperature',
                    heroFont: 'Rajdhani',
                    uiFont: 'Inter',
                    showUnit: false,
                    showLabel: false,
                    showTrend: false,
                    showPeak: false,
                    showArc: false,
                },
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
            const baseAccent = scheme === 'Temperature'
                ? colorByValue(smoothValue, FACE_SCHEMES.temperature)
                : scheme === 'Load'
                  ? colorByValue(smoothValue, FACE_SCHEMES.load)
                  : scheme === 'Memory'
                    ? colorByValue(smoothValue, FACE_SCHEMES.memory)
                    : (controls.customColor as string)
            const secondary = scheme === 'Temperature'
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
                    radius: 134,
                    thickness: 10,
                    value: smoothValue,
                    fillColor: [baseAccent, secondary],
                    trackColor: withAlpha(ink.ui, 0.1),
                    startAngle: Math.PI * 0.98,
                    sweep: Math.PI * 0.86,
                    glow: 0.18 + glow * 0.24,
                })
            } else if (meterStyle === 'scope') {
                arcGauge(c, {
                    cx,
                    cy,
                    radius: 146,
                    thickness: 12,
                    value: smoothValue,
                    fillColor: [baseAccent, secondary],
                    trackColor: withAlpha(ink.ui, 0.1),
                    startAngle: Math.PI * 0.74,
                    sweep: Math.PI * 1.12,
                    glow: 0.2 + glow * 0.28,
                })
            } else {
                arcGauge(c, {
                    cx,
                    cy,
                    radius: 156,
                    thickness: 16,
                    value: smoothValue,
                    fillColor: [baseAccent, secondary],
                    trackColor: withAlpha(ink.ui, 0.12),
                    startAngle: Math.PI * 0.72,
                    sweep: Math.PI * 1.42,
                    glow: 0.24 + glow * 0.32,
                })
            }

            c.restore()
        }
    },
)
