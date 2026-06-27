import type { FaceContext } from '@hypercolor/sdk'
import {
    color,
    combo,
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
import { atmosphereVisible, transparentBackgroundControl } from '../shared/atmosphere'
import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    humanizeSensorLabel,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-pulse-temp'
const HISTORY_LENGTH = 120
const HISTORY_PUSH_INTERVAL = 0.5
const EMBER_COUNT_ROUND = 26
const EMBER_COUNT_WIDE = 44
const HOT_THRESHOLD = 0.75

/** Cool → warm → hot color ramps per scheme. */
const RAMPS: Record<string, [string, string, string]> = {
    Custom: ['#3a3f66', palette.neonCyan, palette.coral],
    Load: ['#2b5d8f', palette.neonCyan, palette.electricYellow],
    Memory: ['#3f3a8f', palette.electricPurple, palette.coral],
    Temperature: ['#2e6b8f', '#a06bff', '#ff5e7a'],
}

const STYLES = `
.hc-pulse-temp {
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --heat: ${palette.neonCyan};
    position: absolute;
    inset: 0;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    color: ${palette.fg.primary};
}

.hc-pulse-temp__stack {
    position: relative;
    z-index: 2;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    text-align: center;
}

.hc-pulse-temp__value {
    display: inline-flex;
    align-items: flex-start;
    justify-content: center;
    font-family: var(--hero-font);
    font-weight: 300;
    line-height: 0.84;
    letter-spacing: -0.01em;
    color: ${palette.fg.primary};
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    text-shadow:
        0 0 32px color-mix(in srgb, var(--heat) 42%, transparent),
        0 0 90px color-mix(in srgb, var(--heat) 22%, transparent);
}

.hc-pulse-temp__unit {
    font-weight: 400;
    margin-top: 0.14em;
    margin-left: 0.06em;
    color: color-mix(in srgb, ${palette.fg.primary} 62%, var(--heat));
}

.hc-pulse-temp__label {
    font-family: var(--ui-font);
    font-weight: 600;
    letter-spacing: 0.34em;
    text-transform: uppercase;
    color: ${palette.fg.tertiary};
    margin-top: 10px;
}

/* ── Wide strip: title-card left, atmosphere flows right ── */

.hc-pulse-temp--wide .hc-pulse-temp__stack {
    position: absolute;
    left: 4.5%;
    top: 50%;
    transform: translateY(-50%);
    align-items: flex-start;
    text-align: left;
}

.hc-pulse-temp--wide .hc-pulse-temp__label {
    margin-top: 4px;
}

.hc-pulse-temp__hidden { display: none !important; }
`

interface Ember {
    seed: number
    lane: number
}

function makeEmbers(count: number): Ember[] {
    return Array.from({ length: count }, (_, index) => ({
        lane: (index * 0.618_03) % 1,
        seed: index * 137.508,
    }))
}

function rampColor(ramp: [string, string, string], t: number): string {
    if (t <= 0.5) return lerpColor(ramp[0], ramp[1], clamp01(t * 2))
    return lerpColor(ramp[1], ramp[2], clamp01((t - 0.5) * 2))
}

function resolveRamp(controls: Record<string, unknown>): [string, string, string] {
    const scheme = (controls.colorScheme as string) ?? 'Temperature'
    if (scheme === 'Custom') {
        const custom = controls.customColor as string
        return ['#3a3f66', custom, lerpColor(custom, palette.coral, 0.55)]
    }
    return RAMPS[scheme] ?? ['#2e6b8f', '#a06bff', '#ff5e7a']
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
        showArc: toggle('Show Scale', true, { group: 'Elements' }),
        showLabel: toggle('Show Label', true, { group: 'Elements' }),
        showNumber: toggle('Show Number', true, { group: 'Elements' }),
        showPeak: toggle('Show Peak', false, { group: 'Elements' }),
        showTrend: toggle('Show Trend', true, { group: 'Elements' }),
        showUnit: toggle('Show Unit', true, { group: 'Elements' }),
        targetSensor: sensor('Sensor', 'cpu_temp', { group: 'Data' }),
        transparentBackground: transparentBackgroundControl(),
        uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography' }),
    },
    {
        author: 'Hypercolor',
        description:
            'A single reading as cinema: a thermal field that grades from cool teal to ember coral, rising heat particles, and title-card type.',
        designBasis: { height: 480, width: 480 },
        presets: [
            {
                controls: { colorScheme: 'Temperature', meterStyle: 'Halo', targetSensor: 'cpu_temp' },
                description: 'The full thermal field with the fine halo scale.',
                name: 'CPU Siren',
            },
            {
                controls: { colorScheme: 'Temperature', meterStyle: 'Vector', targetSensor: 'gpu_temp' },
                description: 'GPU heat with the sweeping vector needle.',
                name: 'GPU Ember',
            },
            {
                controls: { colorScheme: 'Load', meterStyle: 'Halo', targetSensor: 'cpu_load' },
                description: 'CPU load as a cool blue bloom that ignites under pressure.',
                name: 'Load Bloom',
            },
            {
                controls: { colorScheme: 'Memory', meterStyle: 'Scope', targetSensor: 'ram_used' },
                description: 'Memory pressure with the aurora history front and center.',
                name: 'Memory Core',
            },
            {
                controls: { colorScheme: 'Custom', customColor: palette.coral, meterStyle: 'Halo' },
                description: 'Coral-keyed field for warm builds.',
                name: 'Coral Signal',
            },
            {
                controls: {
                    colorScheme: 'Custom',
                    customColor: '#c8d5ff',
                    glowIntensity: 30,
                    meterStyle: 'Vector',
                    showTrend: false,
                },
                description: 'Restrained monochrome with a hairline needle.',
                name: 'Mono Luxe',
            },
            {
                controls: { colorScheme: 'Custom', customColor: '#ffb347', meterStyle: 'Halo' },
                description: 'Amber field, ember-heavy.',
                name: 'Amber Core',
            },
            {
                controls: {
                    glowIntensity: 20,
                    showArc: false,
                    showLabel: false,
                    showTrend: false,
                },
                description: 'Just the numeral floating in the field.',
                name: 'Naked Digit',
            },
        ],
        variants: {
            wide: (ctx: FaceContext) => buildPulseTemp(ctx, true),
        },
    },
    (ctx) => buildPulseTemp(ctx, false),
)

function buildPulseTemp(ctx: FaceContext, wide: boolean) {
    ensureFaceStyles(STYLE_ID, STYLES)
    const root = createFaceRoot(ctx, 'hc-pulse-temp')
    root.classList.toggle('hc-pulse-temp--wide', wide)
    root.innerHTML = `
        <div class="hc-pulse-temp__stack">
            <div class="hc-pulse-temp__value"><span class="hc-pulse-temp__digits">--</span><span class="hc-pulse-temp__unit">°C</span></div>
            <div class="hc-pulse-temp__label">CPU TEMP</div>
        </div>`
    const digitsEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__digits')
    const unitEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__unit')
    const valueEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__value')
    const labelEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__label')
    if (!digitsEl || !unitEl || !valueEl || !labelEl) {
        throw new Error('Pulse Temp face failed to build its DOM')
    }

    const heat = new Smoothed(0, 0.9)
    const displayValue = new Smoothed(0, 0.3)
    const history = new ValueHistory(HISTORY_LENGTH)
    const embers = makeEmbers(wide ? EMBER_COUNT_WIDE : EMBER_COUNT_ROUND)
    let lastTime = Number.NaN
    let lastHistoryPush = Number.NEGATIVE_INFINITY
    let peak = 0

    const scaleBasis = Math.min(ctx.width, ctx.height) / (wide ? 230 : 480)

    return (time: number, controls: Record<string, unknown>, sensors: import('@hypercolor/sdk').SensorAccessor) => {
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time

        const sensorLabel = controls.targetSensor as string
        const reading = sensors.read(sensorLabel)
        const normalized = clamp01(sensors.normalized(sensorLabel))
        const t = heat.update(normalized, dt)
        const ramp = resolveRamp(controls)
        const heatColor = rampColor(ramp, t)
        const glow = clamp01((controls.glowIntensity as number) / 100)
        const hot = clamp01((t - HOT_THRESHOLD) / (1 - HOT_THRESHOLD))
        const pulse = hot > 0 ? hot * (0.5 + 0.5 * Math.sin(time * 2.4)) : 0

        if (time - lastHistoryPush >= HISTORY_PUSH_INTERVAL) {
            lastHistoryPush = time
            history.push(normalized)
        }
        peak = Math.max(peak * (1 - dt * 0.01), normalized)

        // ── Type layer ──
        root.style.setProperty('--heat', heatColor)
        root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
        root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
        const heroSize = (controls.heroSize as number) * scaleBasis
        valueEl.style.fontSize = `${heroSize}px`
        unitEl.style.fontSize = `${heroSize * 0.34}px`
        labelEl.style.fontSize = `${Math.max(9, (controls.detailSize as number) * Math.max(scaleBasis, 0.8))}px`

        const shown = displayValue.update(reading?.value ?? 0, dt)
        digitsEl.textContent = reading ? `${Math.round(shown)}` : '--'
        unitEl.textContent = reading?.unit ?? '°C'
        labelEl.textContent = humanizeSensorLabel(sensorLabel).toUpperCase()
        valueEl.classList.toggle('hc-pulse-temp__hidden', controls.showNumber !== true)
        unitEl.classList.toggle('hc-pulse-temp__hidden', controls.showUnit !== true)
        labelEl.classList.toggle('hc-pulse-temp__hidden', controls.showLabel !== true)

        // ── Atmosphere layer ──
        const c = ctx.ctx
        const W = ctx.width
        const H = ctx.height
        c.clearRect(0, 0, W, H)

        if (atmosphereVisible(controls)) {
            drawThermalField(c, W, H, time, t, ramp, glow, pulse)
            drawEmbers(c, W, H, time, t, embers, heatColor, glow)
        }

        const meterStyle = (controls.meterStyle as string) ?? 'Halo'
        if (controls.showTrend === true) {
            drawAuroraHistory(c, W, H, wide, history.values(), ramp, glow, meterStyle === 'Scope')
        }
        if (controls.showArc === true) {
            if (wide) {
                drawStripScale(c, W, H, t, peak, ramp, heatColor, controls.showPeak === true, glow)
            } else if (meterStyle === 'Vector') {
                drawVectorScale(c, W, H, t, ramp, heatColor, glow)
            } else {
                drawHaloScale(c, W, H, t, peak, ramp, heatColor, controls.showPeak === true, glow)
            }
        }
    }
}

/** Full-bleed atmosphere: graded base, drifting thermal glows, hot pulse. */
function drawThermalField(
    c: CanvasRenderingContext2D,
    W: number,
    H: number,
    time: number,
    t: number,
    ramp: [string, string, string],
    glow: number,
    pulse: number,
): void {
    const horizon = rampColor(ramp, t)
    const deep = rampColor(ramp, Math.max(0, t - 0.35))

    const base = c.createLinearGradient(0, H, 0, 0)
    base.addColorStop(0, withAlpha(horizon, 0.16 + 0.1 * t + 0.08 * pulse))
    base.addColorStop(0.55, withAlpha(deep, 0.05))
    base.addColorStop(1, withAlpha('#05030a', 0))
    c.fillStyle = base
    c.fillRect(0, 0, W, H)

    const blobs = [
        { phase: 0, radius: 0.62, x: 0.24, y: 0.78 },
        { phase: 2.1, radius: 0.5, x: 0.74, y: 0.66 },
        { phase: 4.4, radius: 0.44, x: 0.52, y: 0.3 },
    ]
    for (const blob of blobs) {
        const bx = W * (blob.x + 0.06 * Math.sin(time * 0.11 + blob.phase))
        const by = H * (blob.y + 0.05 * Math.cos(time * 0.09 + blob.phase * 1.7))
        const radius = Math.max(W, H) * blob.radius
        const gradient = c.createRadialGradient(bx, by, 0, bx, by, radius)
        gradient.addColorStop(0, withAlpha(horizon, (0.05 + 0.09 * t) * (0.4 + glow)))
        gradient.addColorStop(1, withAlpha(horizon, 0))
        c.fillStyle = gradient
        c.fillRect(0, 0, W, H)
    }
}

/** Heat particles that rise faster and burn brighter as the reading climbs. */
function drawEmbers(
    c: CanvasRenderingContext2D,
    W: number,
    H: number,
    time: number,
    t: number,
    embers: Ember[],
    heatColor: string,
    glow: number,
): void {
    const activity = 0.15 + t * 0.85
    const visible = Math.max(3, Math.floor(embers.length * activity))
    for (let index = 0; index < visible; index += 1) {
        const ember = embers[index]
        if (!ember) continue
        const speed = 0.025 + activity * 0.085 + (ember.seed % 1) * 0.02
        const progress = (time * speed + ember.seed) % 1
        const sway = Math.sin(time * 0.7 + ember.seed) * W * 0.015
        const x = ember.lane * W + sway
        const y = H * (1.05 - progress * 1.15)
        if (y < -8 || y > H + 8) continue
        const fade = Math.sin(progress * Math.PI)
        const size = 1 + (ember.seed % 2.3) + t * 1.2
        c.fillStyle = withAlpha(heatColor, fade * (0.1 + 0.4 * t) * (0.35 + glow * 0.65))
        c.beginPath()
        c.arc(x, y, size, 0, Math.PI * 2)
        c.fill()
    }
}

/** History as a soft aurora ribbon instead of a clinical sparkline. */
function drawAuroraHistory(
    c: CanvasRenderingContext2D,
    W: number,
    H: number,
    wide: boolean,
    values: number[],
    ramp: [string, string, string],
    glow: number,
    emphasized: boolean,
): void {
    if (values.length < 2) return
    const left = wide ? W * 0.3 : W * 0.14
    const right = wide ? W * 0.97 : W * 0.86
    const baseY = wide ? H * 0.82 : H * 0.78
    const amplitude = (wide ? H * 0.42 : H * 0.16) * (emphasized ? 1.35 : 1)
    const alpha = (emphasized ? 0.34 : 0.18) * (0.5 + glow * 0.5)
    const span = right - left

    c.beginPath()
    c.moveTo(left, baseY)
    for (let index = 0; index < values.length; index += 1) {
        const x = left + (index / (values.length - 1)) * span
        const y = baseY - clamp01(values[index] ?? 0) * amplitude
        c.lineTo(x, y)
    }
    c.lineTo(right, baseY)
    c.closePath()
    const latest = clamp01(values[values.length - 1] ?? 0)
    const ribbon = c.createLinearGradient(0, baseY - amplitude, 0, baseY)
    ribbon.addColorStop(0, withAlpha(rampColor(ramp, latest), alpha))
    ribbon.addColorStop(1, withAlpha(rampColor(ramp, latest), 0))
    c.fillStyle = ribbon
    c.fill()
}

const SCALE_START = Math.PI * 0.78
const SCALE_SWEEP = Math.PI * 1.44

/** Round: a fine tick halo with a glowing comet head at the live value. */
function drawHaloScale(
    c: CanvasRenderingContext2D,
    W: number,
    H: number,
    t: number,
    peak: number,
    ramp: [string, string, string],
    heatColor: string,
    showPeak: boolean,
    glow: number,
): void {
    const cx = W / 2
    const cy = H / 2
    const radius = Math.min(W, H) * 0.41
    const ticks = 72

    for (let index = 0; index < ticks; index += 1) {
        const fraction = index / (ticks - 1)
        const angle = SCALE_START + fraction * SCALE_SWEEP
        const lit = fraction <= t
        const inner = radius - (lit ? 7 : 4)
        const outer = radius + (lit ? 3 : 0)
        c.strokeStyle = lit ? withAlpha(rampColor(ramp, fraction), 0.55 + 0.4 * glow) : withAlpha('#8a8fa8', 0.14)
        c.lineWidth = lit ? 2 : 1
        c.beginPath()
        c.moveTo(cx + Math.cos(angle) * inner, cy + Math.sin(angle) * inner)
        c.lineTo(cx + Math.cos(angle) * outer, cy + Math.sin(angle) * outer)
        c.stroke()
    }

    const headAngle = SCALE_START + t * SCALE_SWEEP
    c.save()
    c.shadowColor = heatColor
    c.shadowBlur = 18 * glow
    c.fillStyle = '#ffffff'
    c.beginPath()
    c.arc(cx + Math.cos(headAngle) * radius, cy + Math.sin(headAngle) * radius, 3.4, 0, Math.PI * 2)
    c.fill()
    c.restore()

    if (showPeak && peak > 0.01) {
        const peakAngle = SCALE_START + clamp01(peak) * SCALE_SWEEP
        c.strokeStyle = withAlpha(palette.electricYellow, 0.8)
        c.lineWidth = 1.5
        c.beginPath()
        c.moveTo(cx + Math.cos(peakAngle) * (radius - 10), cy + Math.sin(peakAngle) * (radius - 10))
        c.lineTo(cx + Math.cos(peakAngle) * (radius + 5), cy + Math.sin(peakAngle) * (radius + 5))
        c.stroke()
    }
}

/** Round alternative: hairline sweep needle over sparse ticks. */
function drawVectorScale(
    c: CanvasRenderingContext2D,
    W: number,
    H: number,
    t: number,
    ramp: [string, string, string],
    heatColor: string,
    glow: number,
): void {
    const cx = W / 2
    const cy = H / 2
    const radius = Math.min(W, H) * 0.41

    for (let index = 0; index < 13; index += 1) {
        const fraction = index / 12
        const angle = SCALE_START + fraction * SCALE_SWEEP
        const major = index % 3 === 0
        const inner = radius - (major ? 10 : 5)
        c.strokeStyle = withAlpha(fraction <= t ? rampColor(ramp, fraction) : '#8a8fa8', fraction <= t ? 0.7 : 0.18)
        c.lineWidth = major ? 2 : 1
        c.beginPath()
        c.moveTo(cx + Math.cos(angle) * inner, cy + Math.sin(angle) * inner)
        c.lineTo(cx + Math.cos(angle) * radius, cy + Math.sin(angle) * radius)
        c.stroke()
    }

    const angle = SCALE_START + t * SCALE_SWEEP
    c.save()
    c.shadowColor = heatColor
    c.shadowBlur = 16 * glow
    c.strokeStyle = withAlpha(heatColor, 0.9)
    c.lineWidth = 2
    c.beginPath()
    c.moveTo(cx + Math.cos(angle) * (radius * 0.55), cy + Math.sin(angle) * (radius * 0.55))
    c.lineTo(cx + Math.cos(angle) * (radius + 2), cy + Math.sin(angle) * (radius + 2))
    c.stroke()
    c.restore()
}

/** Strip: hairline rail with ticks and comet cursor, edge to edge. */
function drawStripScale(
    c: CanvasRenderingContext2D,
    W: number,
    H: number,
    t: number,
    peak: number,
    ramp: [string, string, string],
    heatColor: string,
    showPeak: boolean,
    glow: number,
): void {
    const left = W * 0.3
    const right = W * 0.97
    const y = H * 0.82
    const span = right - left

    c.strokeStyle = withAlpha('#8a8fa8', 0.16)
    c.lineWidth = 1
    c.beginPath()
    c.moveTo(left, y)
    c.lineTo(right, y)
    c.stroke()

    for (let index = 0; index <= 24; index += 1) {
        const fraction = index / 24
        const x = left + fraction * span
        const major = index % 6 === 0
        const lit = fraction <= t
        c.strokeStyle = lit ? withAlpha(rampColor(ramp, fraction), 0.6 + 0.35 * glow) : withAlpha('#8a8fa8', 0.16)
        c.lineWidth = lit ? 2 : 1
        c.beginPath()
        c.moveTo(x, y - (major ? 9 : 5))
        c.lineTo(x, y + (major ? 4 : 2))
        c.stroke()
    }

    const litGradient = c.createLinearGradient(left, 0, right, 0)
    litGradient.addColorStop(0, withAlpha(ramp[0], 0.55))
    litGradient.addColorStop(0.5, withAlpha(ramp[1], 0.55))
    litGradient.addColorStop(1, withAlpha(ramp[2], 0.55))
    c.strokeStyle = litGradient
    c.lineWidth = 2
    c.beginPath()
    c.moveTo(left, y)
    c.lineTo(left + span * t, y)
    c.stroke()

    const hx = left + span * t
    c.save()
    c.shadowColor = heatColor
    c.shadowBlur = 16 * glow
    c.fillStyle = '#ffffff'
    c.beginPath()
    c.arc(hx, y, 3.2, 0, Math.PI * 2)
    c.fill()
    c.restore()

    if (showPeak && peak > 0.01) {
        const px = left + span * clamp01(peak)
        c.strokeStyle = withAlpha(palette.electricYellow, 0.8)
        c.lineWidth = 1.5
        c.beginPath()
        c.moveTo(px, y - 11)
        c.lineTo(px, y + 5)
        c.stroke()
    }
}
