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
    DISPLAY_FONT_FAMILIES,
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

function fontStack(family: string): string {
    return `"${family}", sans-serif`
}

function setTextStyle(c: CanvasRenderingContext2D, family: string, size: number, color: string, weight = 600): void {
    c.font = `${weight} ${size}px ${fontStack(family)}`
    c.fillStyle = color
    c.textBaseline = 'middle'
}

function drawTextWithGlow(
    c: CanvasRenderingContext2D,
    text: string,
    x: number,
    y: number,
    color: string,
    glowColor: string,
    blur: number,
): void {
    c.save()
    c.shadowColor = glowColor
    c.shadowBlur = blur
    c.fillStyle = color
    c.fillText(text, x, y)
    c.restore()
}

function drawCenteredParts(
    c: CanvasRenderingContext2D,
    parts: Array<{ text: string; family: string; size: number; color: string; yOffset?: number }>,
    cx: number,
    cy: number,
    gap: number,
    glowColor: string,
    glowBlur: number,
): void {
    const visible = parts.filter((part) => part.text.length > 0)
    if (visible.length === 0) return

    const widths = visible.map((part) => {
        setTextStyle(c, part.family, part.size, part.color)
        return c.measureText(part.text).width
    })
    const totalWidth = widths.reduce((sum, width) => sum + width, 0) + gap * (visible.length - 1)
    let x = cx - totalWidth * 0.5
    c.textAlign = 'left'
    for (let index = 0; index < visible.length; index += 1) {
        const part = visible[index]
        setTextStyle(c, part.family, part.size, part.color)
        drawTextWithGlow(c, part.text, x, cy + (part.yOffset ?? 0), part.color, glowColor, glowBlur)
        x += widths[index] + gap
    }
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
        for (const child of [...ctx.container.children]) {
            if (child !== ctx.canvas) child.remove()
        }

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

            const glowColor = withAlpha(baseAccent, 0.22 + glow * 0.22)
            const valueParts = [
                ...(showNumber ? [{ color: ink.hero, family: heroFont, size: heroSize, text: numberText }] : []),
                ...(showUnit
                    ? [
                          {
                              color: ink.ui,
                              family: uiFont,
                              size: Math.max(20, Math.min(36, heroSize * 0.22)),
                              text: unitText,
                              yOffset: heroSize * 0.08,
                          },
                      ]
                    : []),
            ]
            drawCenteredParts(c, valueParts, cx, cy, 8, glowColor, 18 + glow * 12)

            c.textAlign = 'center'
            if (showLabel) {
                setTextStyle(c, uiFont, detailSize, ink.ui)
                drawTextWithGlow(c, labelText.toUpperCase(), cx, cy + 74, ink.ui, glowColor, 6 + glow * 5)
            }

            const detailParts = [
                ...(showTrend ? [{ color: ink.ui, family: uiFont, size: detailSize - 1, text: trendText }] : []),
                ...(showPeak ? [{ color: ink.dim, family: uiFont, size: detailSize - 1, text: peakText }] : []),
            ]
            drawCenteredParts(c, detailParts, cx, cy + 100, 18, glowColor, 4 + glow * 4)
            c.restore()
        }
    },
)
