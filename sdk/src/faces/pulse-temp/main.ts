import type { FaceContext, SensorAccessor, SparklineBand } from '@hypercolor/sdk'
import {
    arcGauge,
    clamp,
    color,
    colorByValue,
    combo,
    easeOutCubic,
    face,
    font,
    lerpColor,
    num,
    palette,
    Smoothed,
    sensor,
    sparkline,
    Transition,
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

function canvasFont(size: number, weight: number, family: string): string {
    return `${weight} ${size}px ${fontStack(family)}`
}

function measureText(ctx: CanvasRenderingContext2D, text: string, font: string): number {
    if (text.length === 0) return 0
    ctx.font = font
    return ctx.measureText(text).width
}

function fillGlowingText(
    ctx: CanvasRenderingContext2D,
    text: string,
    x: number,
    y: number,
    fontSpec: string,
    fill: string,
    shadowColor: string,
    shadowBlur: number,
): void {
    if (text.length === 0) return
    ctx.save()
    ctx.font = fontSpec
    ctx.fillStyle = fill
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    ctx.shadowColor = shadowColor
    ctx.shadowBlur = shadowBlur
    ctx.fillText(text, x, y)
    ctx.restore()
}

function drawReadout(
    ctx: CanvasRenderingContext2D,
    cx: number,
    cy: number,
    maxWidth: number,
    numberText: string,
    unitText: string,
    heroFont: string,
    uiFont: string,
    heroSize: number,
    unitSize: number,
    ink: string,
    uiInk: string,
    glowColor: string,
    accent: string,
    glow: number,
): void {
    const numberVisible = numberText.length > 0
    const unitVisible = unitText.length > 0
    if (!numberVisible && !unitVisible) return

    let fittedHeroSize = heroSize
    let fittedUnitSize = unitSize
    let gap = numberVisible && unitVisible ? 8 : 0
    let numberFont = canvasFont(fittedHeroSize, 700, heroFont)
    let unitFont = canvasFont(fittedUnitSize, 600, uiFont)
    let numberWidth = measureText(ctx, numberText, numberFont)
    let unitWidth = measureText(ctx, unitText, unitFont)
    let totalWidth = numberWidth + unitWidth + gap

    if (totalWidth > maxWidth) {
        const scale = maxWidth / totalWidth
        fittedHeroSize = Math.max(42, fittedHeroSize * scale)
        fittedUnitSize = Math.max(16, fittedUnitSize * scale)
        gap *= scale
        numberFont = canvasFont(fittedHeroSize, 700, heroFont)
        unitFont = canvasFont(fittedUnitSize, 600, uiFont)
        numberWidth = measureText(ctx, numberText, numberFont)
        unitWidth = measureText(ctx, unitText, unitFont)
        totalWidth = numberWidth + unitWidth + gap
    }

    let x = cx - totalWidth * 0.5
    if (numberVisible) {
        const numberX = x + numberWidth * 0.5
        fillGlowingText(ctx, numberText, numberX, cy, numberFont, ink, glowColor, 18 + glow * 12)
        fillGlowingText(ctx, numberText, numberX, cy, numberFont, ink, accent, (18 + glow * 12) * 0.42)
        x += numberWidth + gap
    }
    if (unitVisible) {
        fillGlowingText(
            ctx,
            unitText,
            x + unitWidth * 0.5,
            cy + fittedHeroSize * 0.08,
            unitFont,
            uiInk,
            glowColor,
            6 + glow * 5,
        )
    }
}

function drawDetailLine(
    ctx: CanvasRenderingContext2D,
    cx: number,
    y: number,
    trendText: string,
    peakText: string,
    fontSpec: string,
    uiInk: string,
    dimInk: string,
    glowColor: string,
    glow: number,
): void {
    const trendVisible = trendText.length > 0
    const peakVisible = peakText.length > 0
    if (!trendVisible && !peakVisible) return

    const gap = trendVisible && peakVisible ? 18 : 0
    const trendWidth = measureText(ctx, trendText, fontSpec)
    const peakWidth = measureText(ctx, peakText, fontSpec)
    const totalWidth = trendWidth + peakWidth + gap
    let x = cx - totalWidth * 0.5

    if (trendVisible) {
        fillGlowingText(ctx, trendText, x + trendWidth * 0.5, y, fontSpec, uiInk, glowColor, 4 + glow * 4)
        x += trendWidth + gap
    }
    if (peakVisible) {
        fillGlowingText(ctx, peakText, x + peakWidth * 0.5, y, fontSpec, dimInk, glowColor, 4 + glow * 4)
    }
}

// ── Shared per-frame state ──────────────────────────────────────────────

const PULSE_THRESHOLD = 0.78
const PULSE_SECONDS = 0.9
const DRAW_IN_SECONDS = 1.2

interface PulseFrame {
    accent: string
    secondary: string
    ink: ReturnType<typeof resolveFaceInk>
    glow: number
    glowColor: string
    arcValue: number
    smooth: number
    numberText: string
    unitText: string
    labelText: string
    trendText: string
    peakText: string
    history: number[]
    historyCapacity: number
    bands: SparklineBand[]
    pulse: number
    drawIn: number
}

function schemeBands(accent: string, ink: ReturnType<typeof resolveFaceInk>): SparklineBand[] {
    return [
        { color: withAlpha(lerpColor(accent, ink.ui, 0.35), 0.85), min: 0 },
        { color: withAlpha(palette.electricYellow, 0.9), min: 0.65 },
        { color: withAlpha(palette.errorRed, 0.95), min: 0.85 },
    ]
}

function createPulseEngine() {
    const smooth = new Smoothed(0, 0.35)
    const arc = new Transition(0.8)
    let arcPrimed = false
    let lastTime = Number.NaN
    let lastHistoryPush = 0
    let peakReading = Number.NEGATIVE_INFINITY
    let peakDisplay = '--'
    let activeSensor = ''
    let history = new ValueHistory(48)
    let pulseAt = Number.NEGATIVE_INFINITY
    let wasAboveThreshold = false
    let appearedAt = Number.NaN

    return (time: number, controls: Record<string, unknown>, sensors: SensorAccessor): PulseFrame => {
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time
        if (Number.isNaN(appearedAt)) appearedAt = time

        const sensorLabel = controls.targetSensor as string
        const reading = sensors.read(sensorLabel)
        const normalized = sensors.normalized(sensorLabel)

        if (activeSensor !== sensorLabel) {
            activeSensor = sensorLabel
            peakReading = Number.NEGATIVE_INFINITY
            peakDisplay = '--'
            history = new ValueHistory(48)
            wasAboveThreshold = normalized >= PULSE_THRESHOLD
        }

        const value = smooth.update(normalized, dt)
        if (!arcPrimed) {
            arcPrimed = true
            arc.update(0, time)
        }
        const arcValue = arc.update(Math.round(value * 200) / 200, time)

        // Pulse once per upward threshold crossing.
        const above = value >= PULSE_THRESHOLD
        if (above && !wasAboveThreshold) pulseAt = time
        wasAboveThreshold = above
        const pulse = 1 - clamp((time - pulseAt) / PULSE_SECONDS, 0, 1)

        const scheme = controls.colorScheme as string
        let baseAccent =
            scheme === 'Temperature'
                ? colorByValue(value, FACE_SCHEMES.temperature)
                : scheme === 'Load'
                  ? colorByValue(value, FACE_SCHEMES.load)
                  : scheme === 'Memory'
                    ? colorByValue(value, FACE_SCHEMES.memory)
                    : (controls.customColor as string)

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
        const values = history.values()
        const sampleBack = values[Math.max(0, values.length - 8)] ?? value
        const trendDelta = values.length > 8 ? value - sampleBack : 0
        const trendText = trendDelta > 0.018 ? 'RISING' : trendDelta < -0.018 ? 'COOLING' : 'STEADY'

        // Trend nudges the accent: rising leans hot, cooling leans cool.
        if (trendText === 'RISING') baseAccent = lerpColor(baseAccent, palette.coral, 0.16)
        if (trendText === 'COOLING') baseAccent = lerpColor(baseAccent, palette.neonCyan, 0.14)

        const secondary =
            scheme === 'Temperature'
                ? mixFaceAccent(baseAccent, palette.coral, 0.34)
                : scheme === 'Memory'
                  ? mixFaceAccent(baseAccent, palette.electricPurple, 0.48)
                  : mixFaceAccent(baseAccent)
        const ink = resolveFaceInk(baseAccent)
        const glow = clamp01((controls.glowIntensity as number) / 100)

        return {
            accent: baseAccent,
            arcValue,
            bands: schemeBands(baseAccent, ink),
            drawIn: easeOutCubic(clamp((time - appearedAt) / DRAW_IN_SECONDS, 0, 1)),
            glow,
            glowColor: withAlpha(baseAccent, 0.22 + glow * 0.22),
            history: values,
            historyCapacity: 48,
            ink,
            labelText,
            numberText,
            peakText: `PEAK ${peakDisplay}`,
            pulse,
            secondary,
            smooth: value,
            trendText,
            unitText,
        }
    }
}

function drawThresholdSparkline(
    ctx: CanvasRenderingContext2D,
    frame: PulseFrame,
    x: number,
    y: number,
    width: number,
    height: number,
): void {
    if (frame.history.length < 2) return
    // Right-align the rolling window so fresh samples enter from the right.
    const pad = frame.historyCapacity - frame.history.length
    const slotWidth = width / Math.max(frame.historyCapacity - 1, 1)
    sparkline(ctx, {
        bands: frame.bands,
        color: withAlpha(frame.ink.ui, 0.8),
        drawIn: frame.drawIn,
        fill: true,
        fillOpacity: 0.1,
        height,
        lineWidth: 2,
        range: [0, 1],
        values: frame.history,
        width: width - pad * slotWidth,
        x: x + pad * slotWidth,
        y,
    })
}

function drawThresholdPulse(
    ctx: CanvasRenderingContext2D,
    frame: PulseFrame,
    cx: number,
    cy: number,
    baseRadius: number,
): void {
    if (frame.pulse <= 0) return
    const expansion = easeOutCubic(1 - frame.pulse)
    ctx.save()
    ctx.lineWidth = 3 * frame.pulse
    ctx.strokeStyle = withAlpha(frame.accent, 0.5 * frame.pulse)
    ctx.shadowColor = withAlpha(frame.accent, 0.4 * frame.pulse)
    ctx.shadowBlur = 24 * frame.pulse
    ctx.beginPath()
    ctx.arc(cx, cy, baseRadius + expansion * 36, 0, Math.PI * 2)
    ctx.stroke()
    ctx.restore()
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
        variants: {
            wide: (ctx: FaceContext) => {
                const advance = createPulseEngine()
                const { width: W, height: H } = ctx

                return (time, controls, sensors) => {
                    const frame = advance(time, controls, sensors)
                    const uiFont = controls.uiFont as string
                    const heroFont = controls.heroFont as string
                    const detailSize = Math.max(9, Math.min(14, H * 0.08))
                    const showNumber = controls.showNumber as boolean
                    const showUnit = controls.showUnit as boolean
                    const showLabel = controls.showLabel as boolean
                    const showTrend = controls.showTrend as boolean
                    const showPeak = controls.showPeak as boolean

                    const c = ctx.ctx
                    c.clearRect(0, 0, W, H)

                    // Left third: hero readout stacked over its label.
                    const readoutCx = W * 0.16
                    const heroSize = H * 0.46
                    drawReadout(
                        c,
                        readoutCx,
                        H * 0.42,
                        W * 0.28,
                        showNumber ? frame.numberText : '',
                        showUnit ? frame.unitText.toUpperCase() : '',
                        heroFont,
                        uiFont,
                        heroSize,
                        Math.max(14, heroSize * 0.28),
                        frame.ink.hero,
                        frame.ink.ui,
                        frame.glowColor,
                        frame.accent,
                        frame.glow,
                    )
                    if (showLabel) {
                        fillGlowingText(
                            c,
                            frame.labelText.toUpperCase(),
                            readoutCx,
                            H * 0.78,
                            canvasFont(detailSize, 600, uiFont),
                            frame.ink.ui,
                            frame.glowColor,
                            4 + frame.glow * 4,
                        )
                    }

                    // Remaining width: the full-height banded history rail.
                    const railX = W * 0.34
                    const railWidth = W * 0.62
                    const railY = H * 0.18
                    const railHeight = H * 0.58
                    c.save()
                    c.strokeStyle = withAlpha(frame.ink.ui, 0.12)
                    c.lineWidth = 1
                    c.beginPath()
                    c.moveTo(railX, railY + railHeight)
                    c.lineTo(railX + railWidth, railY + railHeight)
                    c.stroke()
                    c.restore()
                    drawThresholdSparkline(c, frame, railX, railY, railWidth, railHeight)
                    drawDetailLine(
                        c,
                        railX + railWidth / 2,
                        H * 0.88,
                        showTrend ? frame.trendText : '',
                        showPeak ? frame.peakText : '',
                        canvasFont(detailSize, 600, uiFont),
                        frame.ink.ui,
                        frame.ink.dim,
                        frame.glowColor,
                        frame.glow,
                    )
                }
            },
        },
    },
    (ctx) => {
        const advance = createPulseEngine()
        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (time, controls, sensors) => {
            const frame = advance(time, controls, sensors)
            const meterStyle = (controls.meterStyle as string).toLowerCase()
            const heroFont = controls.heroFont as string
            const uiFont = controls.uiFont as string
            const heroSize = controls.heroSize as number
            const detailSize = controls.detailSize as number

            const showNumber = controls.showNumber as boolean
            const showUnit = controls.showUnit as boolean
            const showLabel = controls.showLabel as boolean
            const showTrend = controls.showTrend as boolean
            const showPeak = controls.showPeak as boolean
            const showArc = controls.showArc as boolean

            const unitSize = Math.max(20, Math.min(36, heroSize * 0.22))
            const c = ctx.ctx
            c.clearRect(0, 0, W, H)

            c.save()
            if (showArc) {
                c.globalAlpha = 0.92

                if (meterStyle === 'vector') {
                    arcGauge(c, {
                        cx,
                        cy,
                        fillColor: [frame.accent, frame.secondary],
                        glow: 0.18 + frame.glow * 0.24,
                        radius: 134,
                        startAngle: Math.PI * 0.98,
                        sweep: Math.PI * 0.86,
                        thickness: 10,
                        trackColor: withAlpha(frame.ink.ui, 0.1),
                        value: frame.arcValue,
                    })
                    drawThresholdPulse(c, frame, cx, cy, 134)
                } else if (meterStyle === 'scope') {
                    arcGauge(c, {
                        cx,
                        cy,
                        fillColor: [frame.accent, frame.secondary],
                        glow: 0.2 + frame.glow * 0.28,
                        radius: 146,
                        startAngle: Math.PI * 0.74,
                        sweep: Math.PI * 1.12,
                        thickness: 12,
                        trackColor: withAlpha(frame.ink.ui, 0.1),
                        value: frame.arcValue,
                    })
                    drawThresholdPulse(c, frame, cx, cy, 146)
                } else {
                    arcGauge(c, {
                        cx,
                        cy,
                        fillColor: [frame.accent, frame.secondary],
                        glow: 0.24 + frame.glow * 0.32,
                        radius: 156,
                        startAngle: Math.PI * 0.72,
                        sweep: Math.PI * 1.42,
                        thickness: 16,
                        trackColor: withAlpha(frame.ink.ui, 0.12),
                        value: frame.arcValue,
                    })
                    drawThresholdPulse(c, frame, cx, cy, 156)
                }
                c.globalAlpha = 1
            }
            c.restore()

            drawReadout(
                c,
                cx,
                cy,
                W * 0.84,
                showNumber ? frame.numberText : '',
                showUnit ? frame.unitText.toUpperCase() : '',
                heroFont,
                uiFont,
                heroSize,
                unitSize,
                frame.ink.hero,
                frame.ink.ui,
                frame.glowColor,
                frame.accent,
                frame.glow,
            )

            if (showLabel) {
                fillGlowingText(
                    c,
                    frame.labelText.toUpperCase(),
                    cx,
                    cy + 74,
                    canvasFont(detailSize, 600, uiFont),
                    frame.ink.ui,
                    frame.glowColor,
                    6 + frame.glow * 5,
                )
            }

            drawDetailLine(
                c,
                cx,
                cy + 100,
                showTrend ? frame.trendText : '',
                showPeak ? frame.peakText : '',
                canvasFont(Math.max(8, detailSize - 1), 600, uiFont),
                frame.ink.ui,
                frame.ink.dim,
                frame.glowColor,
                frame.glow,
            )

            // The 48-sample history, finally on screen: a banded sparkline
            // tucked into the lower safe area.
            const sparkWidth = Math.min(W * 0.46, 240)
            drawThresholdSparkline(c, frame, cx - sparkWidth / 2, cy + 122, sparkWidth, 36)
        }
    },
)
