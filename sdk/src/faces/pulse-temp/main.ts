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

const FACE_SCHEMES = {
    load: ['#50fa7b', '#00d4ff', '#ff5ca8'] as const,
    memory: ['#77ecff', '#8f70ff'] as const,
    temperature: ['#7ce9ff', '#ffb35f', '#ff6b7a'] as const,
}

const PULSE_TEMP_STYLE_ID = 'hc-pulse-temp-styles'

function fontStack(family: string): string {
    return `"${family}", sans-serif`
}

function setCssVar(element: HTMLElement, name: string, value: string): void {
    if (element.style.getPropertyValue(name) !== value) {
        element.style.setProperty(name, value)
    }
}

function setText(element: HTMLElement, value: string): void {
    if (element.textContent !== value) {
        element.textContent = value
    }
}

function setHidden(element: HTMLElement, hidden: boolean): void {
    element.classList.toggle('hc-pulse-temp__hidden', hidden)
}

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
        ensureFaceStyles(
            PULSE_TEMP_STYLE_ID,
            `
.hc-pulse-temp {
    --accent: #80ffea;
    --dim-ink: rgba(255, 255, 255, 0.56);
    --detail-glow-blur: 8px;
    --detail-small-size: 11px;
    --detail-size: 12px;
    --glow-color: rgba(128, 255, 234, 0.34);
    --hero-font: "Rajdhani", sans-serif;
    --hero-glow-blur: 24px;
    --hero-glow-soft: 10px;
    --hero-ink: #f8fbff;
    --hero-size: 132px;
    --label-glow-blur: 10px;
    --ui-font: "Inter", sans-serif;
    --ui-ink: rgba(236, 244, 255, 0.78);
    --unit-size: 30px;
    --unit-y: 10px;
    box-sizing: border-box;
    contain: layout paint style;
    overflow: hidden;
    text-align: center;
}

.hc-pulse-temp,
.hc-pulse-temp * {
    box-sizing: border-box;
}

.hc-pulse-temp__readout,
.hc-pulse-temp__label,
.hc-pulse-temp__detail {
    left: 50%;
    position: absolute;
    transform: translate(-50%, -50%);
    white-space: nowrap;
}

.hc-pulse-temp__readout {
    align-items: baseline;
    display: flex;
    gap: 8px;
    justify-content: center;
    top: 50%;
    width: 100%;
}

.hc-pulse-temp__number {
    color: var(--hero-ink);
    font-family: var(--hero-font);
    font-size: var(--hero-size);
    font-weight: 700;
    letter-spacing: 0;
    line-height: 0.82;
    text-shadow:
        0 0 var(--hero-glow-blur) var(--glow-color),
        0 0 var(--hero-glow-soft) var(--accent);
}

.hc-pulse-temp__unit {
    color: var(--ui-ink);
    font-family: var(--ui-font);
    font-size: var(--unit-size);
    font-weight: 600;
    letter-spacing: 0;
    line-height: 1;
    text-shadow: 0 0 var(--label-glow-blur) var(--glow-color);
    text-transform: uppercase;
    transform: translateY(var(--unit-y));
}

.hc-pulse-temp__label {
    color: var(--ui-ink);
    font-family: var(--ui-font);
    font-size: var(--detail-size);
    font-weight: 600;
    letter-spacing: 0;
    line-height: 1;
    text-shadow: 0 0 var(--label-glow-blur) var(--glow-color);
    text-transform: uppercase;
    top: calc(50% + 74px);
}

.hc-pulse-temp__detail {
    align-items: center;
    color: var(--ui-ink);
    display: flex;
    font-family: var(--ui-font);
    font-size: var(--detail-small-size);
    font-weight: 600;
    gap: 18px;
    justify-content: center;
    letter-spacing: 0;
    line-height: 1;
    text-shadow: 0 0 var(--detail-glow-blur) var(--glow-color);
    top: calc(50% + 100px);
}

.hc-pulse-temp__peak {
    color: var(--dim-ink);
}

.hc-pulse-temp__hidden {
    display: none !important;
}
`,
        )

        const root = createFaceRoot(ctx, 'hc-pulse-temp')
        const readoutElement = document.createElement('div')
        const numberElement = document.createElement('span')
        const unitElement = document.createElement('span')
        const labelElement = document.createElement('div')
        const detailElement = document.createElement('div')
        const trendElement = document.createElement('span')
        const peakElement = document.createElement('span')

        readoutElement.className = 'hc-pulse-temp__readout'
        numberElement.className = 'hc-pulse-temp__number'
        unitElement.className = 'hc-pulse-temp__unit'
        labelElement.className = 'hc-pulse-temp__label'
        detailElement.className = 'hc-pulse-temp__detail'
        trendElement.className = 'hc-pulse-temp__trend'
        peakElement.className = 'hc-pulse-temp__peak'

        readoutElement.append(numberElement, unitElement)
        detailElement.append(trendElement, peakElement)
        root.append(readoutElement, labelElement, detailElement)

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
            const heroFont = controls.heroFont as string
            const uiFont = controls.uiFont as string
            const heroSize = controls.heroSize as number
            const detailSize = controls.detailSize as number

            const formatted = sensors.formatted(sensorLabel)
            const match = formatted.match(/^([\d.]+)\s*(.*)$/)
            const numberText = match?.[1] ?? formatted
            const unitText = match?.[2] || (reading?.unit ?? '')
            const labelText = humanizeSensorLabel(sensorLabel)

            if (time - lastHistoryPush > 0.12) {
                history.push(normalized)
                lastHistoryPush = time
            }
            if (reading?.value != null && reading.value >= peakReading) {
                peakReading = reading.value
                peakDisplay = formatted
            }
            const peakText = `PEAK ${peakDisplay}`
            const values = history.values()
            const trendDelta = values.length > 8 ? smoothValue - values[Math.max(0, values.length - 8)] : 0
            const trendText = trendDelta > 0.018 ? 'RISING' : trendDelta < -0.018 ? 'COOLING' : 'STEADY'

            const showNumber = controls.showNumber as boolean
            const showUnit = controls.showUnit as boolean
            const showLabel = controls.showLabel as boolean
            const showTrend = controls.showTrend as boolean
            const showPeak = controls.showPeak as boolean
            const showArc = controls.showArc as boolean

            const glowColor = withAlpha(baseAccent, 0.22 + glow * 0.22)
            const unitSize = Math.max(20, Math.min(36, heroSize * 0.22))
            setCssVar(root, '--accent', baseAccent)
            setCssVar(root, '--dim-ink', ink.dim)
            setCssVar(root, '--detail-glow-blur', `${4 + glow * 4}px`)
            setCssVar(root, '--detail-size', `${detailSize}px`)
            setCssVar(root, '--detail-small-size', `${Math.max(8, detailSize - 1)}px`)
            setCssVar(root, '--glow-color', glowColor)
            setCssVar(root, '--hero-font', fontStack(heroFont))
            setCssVar(root, '--hero-glow-blur', `${18 + glow * 12}px`)
            setCssVar(root, '--hero-glow-soft', `${(18 + glow * 12) * 0.42}px`)
            setCssVar(root, '--hero-ink', ink.hero)
            setCssVar(root, '--hero-size', `${heroSize}px`)
            setCssVar(root, '--label-glow-blur', `${6 + glow * 5}px`)
            setCssVar(root, '--ui-font', fontStack(uiFont))
            setCssVar(root, '--ui-ink', ink.ui)
            setCssVar(root, '--unit-size', `${unitSize}px`)
            setCssVar(root, '--unit-y', `${heroSize * 0.08}px`)

            setText(numberElement, numberText)
            setText(unitElement, unitText)
            setText(labelElement, labelText.toUpperCase())
            setText(trendElement, trendText)
            setText(peakElement, peakText)

            const numberVisible = showNumber && numberText.length > 0
            const unitVisible = showUnit && unitText.length > 0
            const trendVisible = showTrend && trendText.length > 0
            const peakVisible = showPeak && peakText.length > 0
            setHidden(readoutElement, !numberVisible && !unitVisible)
            setHidden(numberElement, !numberVisible)
            setHidden(unitElement, !unitVisible)
            setHidden(labelElement, !showLabel)
            setHidden(detailElement, !trendVisible && !peakVisible)
            setHidden(trendElement, !trendVisible)
            setHidden(peakElement, !peakVisible)

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)

            c.save()
            if (showArc) {
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
                c.globalAlpha = 1
            }
            c.restore()
        }
    },
)
