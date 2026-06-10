import type { FaceContext } from '@hypercolor/sdk'
import { color, combo, easeOutCubic, face, font, lerpColor, num, palette, toggle, withAlpha } from '@hypercolor/sdk'

import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    mixFaceAccent,
    resolveFaceInk,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-neon-clock'
const DIGIT_MORPH_SECONDS = 0.45

const STYLES = `
.hc-neon-clock {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --headline-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --time-size: 120;
    --meta-size: 12;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
}

.hc-neon-clock__time {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -58%);
    display: inline-flex;
    flex-direction: row;
    align-items: flex-end;
    justify-content: center;
    gap: 10px;
    font-family: var(--headline-font);
    font-size: calc(var(--time-size) * 1px);
    font-weight: 600;
    line-height: 1;
    letter-spacing: 0.015em;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    white-space: nowrap;
    text-shadow:
        0 0 20px color-mix(in srgb, var(--accent) 12%, transparent),
        0 10px 28px rgba(0, 0, 0, 0.28);
}

.hc-neon-clock__slot {
    display: inline-flex;
    flex-direction: row;
    justify-content: center;
}

.hc-neon-clock__digit {
    position: relative;
    display: inline-flex;
    width: 0.68ch;
    height: 1em;
    justify-content: center;
    overflow: hidden;
}

.hc-neon-clock__digit-layer {
    position: absolute;
    inset: 0;
    display: flex;
    justify-content: center;
    will-change: transform, opacity;
}

.hc-neon-clock__digit--blank {
    opacity: 0;
}

.hc-neon-clock__separator {
    color: var(--dim-ink);
    transform: translateY(-3px);
}

.hc-neon-clock__meta {
    position: absolute;
    top: calc(50% + 22px);
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 14px;
    min-height: 1em;
    font-family: var(--ui-font);
    font-size: calc(var(--meta-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--ui-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    white-space: nowrap;
}

.hc-neon-clock__seconds,
.hc-neon-clock__ampm {
    color: var(--dim-ink);
}

.hc-neon-clock__hidden {
    display: none !important;
}

.hc-neon-clock[data-style='split'] .hc-neon-clock__time {
    transform: translate(-50%, -56%);
}

.hc-neon-clock[data-style='pulse'] .hc-neon-clock__meta {
    top: calc(50% + 26px);
}

/* ── Wide strip layout ── */

.hc-neon-clock--wide .hc-neon-clock__time {
    position: static;
    transform: none;
}

.hc-neon-clock--wide .hc-neon-clock__meta {
    position: static;
    transform: none;
    flex-direction: column;
    align-items: flex-end;
    gap: 4px;
}

.hc-neon-clock--wide {
    display: flex;
    flex-direction: row;
    align-items: center;
    justify-content: space-between;
    padding: 0 6%;
    box-sizing: border-box;
}
`

// ── Digit morphing ──────────────────────────────────────────────────────

interface DigitSlot {
    root: HTMLSpanElement
    current: HTMLSpanElement
    outgoing: HTMLSpanElement
    value: string | null
    changedAt: number
}

function buildDigit(className: string): { html: string } {
    return {
        html: `<span class="hc-neon-clock__digit ${className}">
            <span class="hc-neon-clock__digit-layer hc-neon-clock__digit-current">0</span>
            <span class="hc-neon-clock__digit-layer hc-neon-clock__digit-outgoing" style="opacity:0">0</span>
        </span>`,
    }
}

function bindDigit(root: HTMLElement, className: string): DigitSlot {
    const slot = root.querySelector<HTMLSpanElement>(`.${className}`)
    if (!slot) throw new Error(`missing digit slot ${className}`)
    return {
        changedAt: Number.NEGATIVE_INFINITY,
        current: slot.querySelector<HTMLSpanElement>('.hc-neon-clock__digit-current') as HTMLSpanElement,
        outgoing: slot.querySelector<HTMLSpanElement>('.hc-neon-clock__digit-outgoing') as HTMLSpanElement,
        root: slot,
        value: null,
    }
}

function setDigit(slot: DigitSlot, value: string | null, time: number): void {
    if (slot.value === value) return
    slot.outgoing.textContent = slot.value ?? value ?? '0'
    slot.current.textContent = value ?? '0'
    // Skip the morph on the very first paint so the clock doesn't cascade.
    slot.changedAt = slot.value === null ? Number.NEGATIVE_INFINITY : time
    slot.value = value
    slot.root.classList.toggle('hc-neon-clock__digit--blank', value == null)
}

function animateDigit(slot: DigitSlot, time: number): void {
    const progress = clamp01((time - slot.changedAt) / DIGIT_MORPH_SECONDS)
    const eased = easeOutCubic(progress)
    if (progress >= 1) {
        slot.current.style.opacity = '1'
        slot.current.style.transform = 'translateY(0)'
        slot.outgoing.style.opacity = '0'
        return
    }
    slot.current.style.opacity = `${eased}`
    slot.current.style.transform = `translateY(${(1 - eased) * 0.32}em)`
    slot.outgoing.style.opacity = `${1 - eased}`
    slot.outgoing.style.transform = `translateY(${-eased * 0.32}em)`
}

interface ClockDom {
    root: HTMLDivElement
    timeEl: HTMLDivElement
    separatorEl: HTMLSpanElement
    hoursTens: DigitSlot
    hoursOnes: DigitSlot
    minutesTens: DigitSlot
    minutesOnes: DigitSlot
    secondsEl: HTMLSpanElement
    dateEl: HTMLSpanElement
    ampmEl: HTMLSpanElement
    metaEl: HTMLDivElement
}

function buildClockDom(ctx: FaceContext, wide: boolean): ClockDom {
    ensureFaceStyles(STYLE_ID, STYLES)
    const root = createFaceRoot(ctx, 'hc-neon-clock')
    root.classList.toggle('hc-neon-clock--wide', wide)
    root.innerHTML = `
        <div class="hc-neon-clock__time">
            <span class="hc-neon-clock__slot hc-neon-clock__slot--hours">
                ${buildDigit('hc-neon-clock__hours-tens').html}
                ${buildDigit('hc-neon-clock__hours-ones').html}
            </span>
            <span class="hc-neon-clock__separator">:</span>
            <span class="hc-neon-clock__slot hc-neon-clock__slot--minutes">
                ${buildDigit('hc-neon-clock__minutes-tens').html}
                ${buildDigit('hc-neon-clock__minutes-ones').html}
            </span>
        </div>
        <div class="hc-neon-clock__meta">
            <span class="hc-neon-clock__date"></span>
            <span class="hc-neon-clock__seconds"></span>
            <span class="hc-neon-clock__ampm"></span>
        </div>
    `

    const query = <T extends HTMLElement>(selector: string): T => {
        const found = root.querySelector<T>(selector)
        if (!found) throw new Error(`missing clock element ${selector}`)
        return found
    }

    return {
        ampmEl: query('.hc-neon-clock__ampm'),
        dateEl: query('.hc-neon-clock__date'),
        hoursOnes: bindDigit(root, 'hc-neon-clock__hours-ones'),
        hoursTens: bindDigit(root, 'hc-neon-clock__hours-tens'),
        metaEl: query('.hc-neon-clock__meta'),
        minutesOnes: bindDigit(root, 'hc-neon-clock__minutes-ones'),
        minutesTens: bindDigit(root, 'hc-neon-clock__minutes-tens'),
        root,
        secondsEl: query('.hc-neon-clock__seconds'),
        separatorEl: query('.hc-neon-clock__separator'),
        timeEl: query('.hc-neon-clock__time'),
    }
}

interface ClockFrame {
    accent: string
    ink: ReturnType<typeof resolveFaceInk>
    glow: number
    breath: number
    secondsSweep: number
    seconds: number
    is12h: boolean
    showSeconds: boolean
    showDate: boolean
    showAmPm: boolean
    showTime: boolean
    showSeparator: boolean
    dialStyle: string
}

/** Shared per-frame DOM update; returns everything the dials need. */
function updateClockDom(dom: ClockDom, time: number, controls: Record<string, unknown>): ClockFrame {
    const accent = lerpColor(controls.accent as string, palette.fg.primary, 0.06)
    const secondary = mixFaceAccent(controls.secondaryAccent as string, accent, 0.12)
    const ink = resolveFaceInk(accent)
    const glow = clamp01((controls.glowIntensity as number) / 100)
    const showTime = controls.showTime as boolean
    const showSeparator = controls.showSeparator as boolean
    const showSeconds = controls.showSeconds as boolean
    const showDate = controls.showDate as boolean
    const showAmPm = controls.showAmPm as boolean
    const is12h = controls.hourFormat === '12h'
    const dialStyle = (controls.dialStyle as string).toLowerCase()
    const { root } = dom

    root.dataset.style = dialStyle
    root.style.setProperty('--accent', accent)
    root.style.setProperty('--secondary', secondary)
    root.style.setProperty('--hero-ink', ink.hero)
    root.style.setProperty('--ui-ink', ink.ui)
    root.style.setProperty('--dim-ink', ink.dim)
    root.style.setProperty('--headline-font', `"${controls.headlineFont as string}", sans-serif`)
    root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
    root.style.setProperty('--meta-size', `${controls.metaSize as number}`)

    const now = new Date()
    let hours = now.getHours()
    const minutes = now.getMinutes()
    const seconds = now.getSeconds()
    const millis = now.getMilliseconds()
    const ampm = hours >= 12 ? 'PM' : 'AM'
    if (is12h) hours = hours % 12 || 12

    const hoursText = hours.toString()
    const minutesText = minutes.toString().padStart(2, '0')
    setDigit(dom.hoursTens, hoursText.length > 1 ? (hoursText[0] ?? null) : null, time)
    setDigit(dom.hoursOnes, hoursText[hoursText.length - 1] ?? '0', time)
    setDigit(dom.minutesTens, minutesText[0] ?? '0', time)
    setDigit(dom.minutesOnes, minutesText[1] ?? '0', time)
    for (const slot of [dom.hoursTens, dom.hoursOnes, dom.minutesTens, dom.minutesOnes]) {
        animateDigit(slot, time)
    }

    dom.secondsEl.textContent = `SEC ${seconds.toString().padStart(2, '0')}`
    dom.ampmEl.textContent = is12h ? ampm : ''
    dom.dateEl.textContent = now
        .toLocaleDateString('en-US', { day: 'numeric', month: 'short', weekday: 'short' })
        .toUpperCase()

    dom.timeEl.classList.toggle('hc-neon-clock__hidden', !showTime)
    dom.separatorEl.classList.toggle('hc-neon-clock__hidden', !showSeparator)
    dom.dateEl.classList.toggle('hc-neon-clock__hidden', !showDate)
    dom.secondsEl.classList.toggle('hc-neon-clock__hidden', !showSeconds)
    dom.ampmEl.classList.toggle('hc-neon-clock__hidden', !is12h || !showAmPm)
    dom.metaEl.classList.toggle('hc-neon-clock__hidden', !showDate && !showSeconds && !(is12h && showAmPm))

    // Subtle breathing on the glow — slow enough to stay graceful at 15fps.
    const breath = 0.5 + 0.5 * Math.sin(time * 0.9)
    return {
        accent,
        breath,
        dialStyle,
        glow,
        ink,
        is12h,
        seconds,
        secondsSweep: (seconds + millis / 1000) / 60,
        showAmPm,
        showDate,
        showSeconds,
        showSeparator,
        showTime,
    }
}

// ── Controls + presets (unchanged contract) ─────────────────────────────

const CONTROLS = {
    accent: color('Accent', palette.neonCyan, { group: 'Style' }),
    dialStyle: combo('Dial Style', ['Orbit', 'Split', 'Pulse'], { group: 'Clock' }),
    glowIntensity: num('Glow', [0, 100], 56, { group: 'Style' }),
    headlineFont: font('Headline Font', 'Rajdhani', { families: [...DISPLAY_FONT_FAMILIES], group: 'Typography' }),
    hourFormat: combo('Format', ['24h', '12h'], { group: 'Clock' }),
    metaSize: num('Meta Size', [8, 24], 12, { group: 'Typography' }),
    secondaryAccent: color('Secondary', palette.electricPurple, { group: 'Style' }),
    showAmPm: toggle('Show AM/PM', true, { group: 'Elements' }),
    showDate: toggle('Show Date', true, { group: 'Elements' }),
    showDial: toggle('Show Dial', true, { group: 'Elements' }),
    showSeconds: toggle('Show Seconds', false, { group: 'Elements' }),
    showSeparator: toggle('Show Separator', true, { group: 'Elements' }),
    showTime: toggle('Show Time', true, { group: 'Elements' }),
    timeSize: num('Time Size', [72, 164], 120, { group: 'Typography' }),
    uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography' }),
}

export default face(
    'Neon Clock',
    CONTROLS,
    {
        author: 'Hypercolor',
        description: 'A centered digital clock. Every element is independently toggleable.',
        designBasis: { height: 480, width: 480 },
        presets: [
            {
                controls: {
                    accent: palette.neonCyan,
                    dialStyle: 'Orbit',
                    glowIntensity: 60,
                    headlineFont: 'Rajdhani',
                    secondaryAccent: palette.electricPurple,
                    uiFont: 'Inter',
                },
                description: 'Cyan-violet clock with balanced tech numerals.',
                name: 'Electric Midnight',
            },
            {
                controls: {
                    accent: palette.coral,
                    dialStyle: 'Pulse',
                    glowIntensity: 54,
                    headlineFont: 'Exo 2',
                    secondaryAccent: '#ffb3f2',
                    uiFont: 'DM Sans',
                },
                description: 'Soft coral and purple with crisp modern type.',
                name: 'Blush Circuit',
            },
            {
                controls: {
                    accent: palette.electricYellow,
                    dialStyle: 'Split',
                    glowIntensity: 48,
                    headlineFont: 'Space Mono',
                    secondaryAccent: '#ff8d4d',
                    uiFont: 'JetBrains Mono',
                },
                description: 'Monospaced clock with clean amber accents.',
                name: 'Arcade Mono',
            },
            {
                controls: {
                    accent: '#ffb38a',
                    dialStyle: 'Pulse',
                    glowIntensity: 42,
                    headlineFont: 'Rajdhani',
                    secondaryAccent: '#ffd2c3',
                    uiFont: 'Roboto Condensed',
                },
                description: 'Warm rose-gold with compact numerals.',
                name: 'Afterglow',
            },
            {
                controls: {
                    accent: '#9ae7ff',
                    dialStyle: 'Orbit',
                    glowIntensity: 44,
                    headlineFont: 'Exo 2',
                    secondaryAccent: '#d6ecff',
                    uiFont: 'Inter',
                },
                description: 'Cool blue-white with airy accents.',
                name: 'Frostline',
            },
            {
                controls: {
                    accent: '#ff4da6',
                    dialStyle: 'Split',
                    glowIntensity: 58,
                    headlineFont: 'Rajdhani',
                    secondaryAccent: '#6a8bff',
                    uiFont: 'Space Grotesk',
                },
                description: 'Magenta-blue contrast with strong readable rhythm.',
                name: 'Night Drive',
            },
            {
                controls: {
                    accent: '#8ef4ff',
                    dialStyle: 'Orbit',
                    glowIntensity: 50,
                    headlineFont: 'Orbitron',
                    secondaryAccent: '#ffc76b',
                    uiFont: 'DM Sans',
                },
                description: 'Cyan and amber with subtle orbit accent.',
                name: 'Signal Drift',
            },
            {
                controls: {
                    accent: palette.neonCyan,
                    headlineFont: 'Rajdhani',
                    secondaryAccent: palette.electricPurple,
                    showAmPm: false,
                    showDate: false,
                    showDial: false,
                    showSeconds: false,
                    uiFont: 'Inter',
                },
                description: 'Just the digits. No meta, no dial.',
                name: 'Naked Time',
            },
        ],
        variants: {
            wide: (ctx) => {
                const dom = buildClockDom(ctx, true)
                const { width: W, height: H } = ctx

                return (time, controls) => {
                    const frame = updateClockDom(dom, time, controls)
                    // Strip sizing keys off the panel height, scaled by the
                    // author's time-size preference around its 120 default.
                    const timeScale = (controls.timeSize as number) / 120
                    dom.root.style.setProperty('--time-size', `${H * 0.56 * timeScale}`)

                    const c = ctx.ctx
                    c.clearRect(0, 0, W, H)
                    if (!(controls.showDial as boolean)) return

                    // Eased seconds underline in place of the dial: a thin
                    // accent rail under the digits that fills each minute.
                    const margin = W * 0.06
                    const railY = H * 0.86
                    const railWidth = W - margin * 2
                    c.save()
                    c.lineCap = 'round'
                    c.lineWidth = 3
                    c.strokeStyle = withAlpha(frame.accent, 0.1)
                    c.beginPath()
                    c.moveTo(margin, railY)
                    c.lineTo(margin + railWidth, railY)
                    c.stroke()

                    const sweep = easeOutCubic(frame.secondsSweep)
                    c.strokeStyle = withAlpha(frame.accent, 0.45 + frame.glow * 0.3 + frame.breath * 0.1)
                    c.shadowColor = withAlpha(frame.accent, 0.3 + frame.glow * 0.3)
                    c.shadowBlur = 8 + frame.glow * 10
                    c.beginPath()
                    c.moveTo(margin, railY)
                    c.lineTo(margin + railWidth * sweep, railY)
                    c.stroke()
                    c.restore()
                }
            },
        },
    },
    (ctx) => {
        const dom = buildClockDom(ctx, false)
        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (time, controls) => {
            const frame = updateClockDom(dom, time, controls)
            dom.root.style.setProperty('--time-size', `${controls.timeSize as number}`)

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)
            if (!(controls.showDial as boolean)) return

            const breathGlow = frame.glow * (0.85 + frame.breath * 0.3)
            c.save()
            c.strokeStyle = withAlpha(frame.accent, 0.16 + breathGlow * 0.14)
            c.shadowColor = withAlpha(frame.accent, 0.16 + breathGlow * 0.12)
            c.shadowBlur = 20 + breathGlow * 24

            if (frame.dialStyle === 'split') {
                c.lineWidth = 3
                c.lineCap = 'round'
                c.beginPath()
                c.moveTo(cx - 112, cy + 78)
                c.lineTo(cx - 36, cy + 78)
                c.moveTo(cx + 36, cy + 78)
                c.lineTo(cx + 112, cy + 78)
                c.stroke()

                // Seconds tick gliding along the split rails.
                const sweep = easeOutCubic(frame.secondsSweep)
                const tickX = sweep < 0.5 ? cx - 112 + (76 * sweep) / 0.5 : cx + 36 + (76 * (sweep - 0.5)) / 0.5
                c.fillStyle = withAlpha(frame.accent, 0.7 + frame.breath * 0.2)
                c.beginPath()
                c.arc(tickX, cy + 78, 4, 0, Math.PI * 2)
                c.fill()
            } else if (frame.dialStyle === 'pulse') {
                c.lineWidth = 4
                c.lineCap = 'round'
                c.beginPath()
                c.moveTo(cx - 92, cy + 74)
                c.quadraticCurveTo(cx, cy + 94, cx + 92, cy + 74)
                c.stroke()

                // Seconds dot tracing the pulse curve.
                const t = frame.secondsSweep
                const mt = 1 - t
                const dotX = mt * mt * (cx - 92) + 2 * mt * t * cx + t * t * (cx + 92)
                const dotY = mt * mt * (cy + 74) + 2 * mt * t * (cy + 94) + t * t * (cy + 74)
                c.fillStyle = withAlpha(frame.accent, 0.7 + frame.breath * 0.2)
                c.beginPath()
                c.arc(dotX, dotY, 4.5, 0, Math.PI * 2)
                c.fill()
            } else {
                // Orbit: decorative arc + a full sweep-second ring.
                c.lineWidth = 5
                c.lineCap = 'round'
                c.beginPath()
                c.arc(cx, cy + 8, 122, Math.PI * 0.78, Math.PI * 1.28)
                c.stroke()

                const ringRadius = 122
                c.lineWidth = 1.5
                c.strokeStyle = withAlpha(frame.accent, 0.07)
                c.shadowBlur = 0
                c.beginPath()
                c.arc(cx, cy + 8, ringRadius, 0, Math.PI * 2)
                c.stroke()

                const angle = -Math.PI / 2 + frame.secondsSweep * Math.PI * 2
                // Trailing comet: three fading arc segments behind the dot.
                for (let trail = 0; trail < 3; trail++) {
                    const span = 0.05 * Math.PI * 2
                    const end = angle - trail * span
                    c.lineWidth = 3 - trail * 0.8
                    c.strokeStyle = withAlpha(frame.accent, (0.4 - trail * 0.12) * (0.7 + frame.breath * 0.3))
                    c.beginPath()
                    c.arc(cx, cy + 8, ringRadius, end - span, end)
                    c.stroke()
                }

                const dotX = cx + Math.cos(angle) * ringRadius
                const dotY = cy + 8 + Math.sin(angle) * ringRadius
                c.fillStyle = withAlpha(frame.accent, 0.85)
                c.shadowColor = withAlpha(frame.accent, 0.5 + frame.glow * 0.3)
                c.shadowBlur = 10 + breathGlow * 12
                c.beginPath()
                c.arc(dotX, dotY, 4.5, 0, Math.PI * 2)
                c.fill()
            }

            c.restore()
        }
    },
)
