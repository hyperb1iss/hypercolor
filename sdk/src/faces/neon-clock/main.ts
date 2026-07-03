import type { FaceContext } from '@hypercolor/sdk'
import { color, combo, easeOutCubic, face, font, num, palette, toggle, withAlpha } from '@hypercolor/sdk'

import {
    atmosphereVisible,
    drawCometRail,
    drawCometRing,
    drawNebulaField,
    drawRisingMotes,
    entrance,
    makeDrifters,
    transparentBackgroundControl,
} from '../shared/atmosphere'
import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    resolveFaceInk,
    SmoothedColor,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-neon-clock'

const STYLES = `
.hc-neon-clock {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --time-size: 150;
    --meta-size: 12;
    --digit-gap: 0.04;
    position: absolute;
    inset: 0;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--hero-ink);
}

.hc-neon-clock__stack {
    position: relative;
    z-index: 2;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
}

.hc-neon-clock__time {
    display: inline-flex;
    align-items: baseline;
    justify-content: center;
    gap: calc(var(--digit-gap) * 1em);
    font-family: var(--hero-font);
    font-size: calc(var(--time-size) * 1px);
    font-weight: 500;
    line-height: 0.84;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    text-shadow:
        0 0 30px color-mix(in srgb, var(--accent) 38%, transparent),
        0 0 90px color-mix(in srgb, var(--secondary) 20%, transparent);
}

.hc-neon-clock__digit {
    display: inline-flex;
    width: 0.6ch;
    justify-content: center;
    will-change: transform, opacity;
}

.hc-neon-clock__digit--blank { opacity: 0; }

.hc-neon-clock__separator {
    color: color-mix(in srgb, var(--accent) 70%, transparent);
    transform: translateY(-0.04em);
    will-change: opacity;
}

.hc-neon-clock__meta {
    display: flex;
    flex-direction: row;
    align-items: center;
    gap: 14px;
    font-family: var(--ui-font);
    font-size: calc(var(--meta-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.3em;
    text-transform: uppercase;
    color: var(--ui-ink);
    will-change: transform, opacity;
}

.hc-neon-clock__ampm {
    padding: 3px 8px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
    color: color-mix(in srgb, var(--hero-ink) 75%, var(--accent));
    letter-spacing: 0.18em;
}

.hc-neon-clock__seconds {
    color: color-mix(in srgb, var(--accent) 80%, transparent);
    font-variant-numeric: tabular-nums;
}

/* ── Wide strip: digits left, meta right, sky in between ── */

.hc-neon-clock--wide .hc-neon-clock__stack {
    position: absolute;
    left: 4%;
    top: 50%;
    transform: translateY(-50%);
    flex-direction: row;
    align-items: baseline;
    gap: 26px;
}

.hc-neon-clock--wide .hc-neon-clock__meta {
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
}

.hc-neon-clock__hidden { display: none !important; }
`

const CONTROLS = {
    accent: color('Accent', palette.neonCyan, { group: 'Style' }),
    dialStyle: combo('Dial Style', ['Orbit', 'Split', 'Pulse'], { group: 'Clock' }),
    digitSpacing: num('Digit Spacing', [0, 50], 4, { group: 'Typography' }),
    glowIntensity: num('Glow', [0, 100], 56, { group: 'Style' }),
    headlineFont: font('Headline Font', 'Rajdhani', {
        families: [...DISPLAY_FONT_FAMILIES],
        group: 'Typography',
        weights: [500],
    }),
    hourFormat: combo('Format', ['24h', '12h'], { group: 'Clock' }),
    metaSize: num('Meta Size', [8, 24], 12, { group: 'Typography' }),
    secondaryAccent: color('Secondary', palette.electricPurple, { group: 'Style' }),
    showAmPm: toggle('Show AM/PM', true, { group: 'Elements' }),
    showDate: toggle('Show Date', true, { group: 'Elements' }),
    showDial: toggle('Show Dial', true, { group: 'Elements' }),
    showSeconds: toggle('Show Seconds', false, { group: 'Elements' }),
    showSeparator: toggle('Show Separator', true, { group: 'Elements' }),
    showTime: toggle('Show Time', true, { group: 'Elements' }),
    timeSize: num('Time Size', [72, 164], 150, { group: 'Typography' }),
    transparentBackground: transparentBackgroundControl(),
    uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography', weights: [600] }),
}

export default face(
    'Neon Clock',
    CONTROLS,
    {
        author: 'Hypercolor',
        description:
            'Time as the whole picture: monumental digits over a drifting nebula, with an orbiting second comet.',
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
            wide: (ctx: FaceContext) => buildNeonClock(ctx, true),
        },
    },
    (ctx) => buildNeonClock(ctx, false),
)

/** Eases a digit swap: the new glyph rises in as the old value leaves. */
function createDigitMorph(element: HTMLElement) {
    let last = ''
    let changedAt = Number.NEGATIVE_INFINITY
    return (text: string, blank: boolean, time: number) => {
        if (text !== last) {
            last = text
            changedAt = time
            element.textContent = text
        }
        element.classList.toggle('hc-neon-clock__digit--blank', blank)
        if (blank) return
        const progress = clamp01((time - changedAt) / 0.5)
        const eased = easeOutCubic(progress)
        element.style.opacity = `${0.25 + 0.75 * eased}`
        element.style.transform = `translateY(${(1 - eased) * 4.3}px) scale(${0.96 + eased * 0.04})`
        if (progress >= 1) {
            element.style.opacity = '1'
            element.style.transform = 'translateY(0) scale(1)'
        }
    }
}

function buildNeonClock(ctx: FaceContext, wide: boolean) {
    ensureFaceStyles(STYLE_ID, STYLES)
    const root = createFaceRoot(ctx, 'hc-neon-clock')
    root.classList.toggle('hc-neon-clock--wide', wide)
    root.innerHTML = `
        <div class="hc-neon-clock__stack">
            <div class="hc-neon-clock__time">
                <span class="hc-neon-clock__digit hc-neon-clock__h0">0</span>
                <span class="hc-neon-clock__digit hc-neon-clock__h1">0</span>
                <span class="hc-neon-clock__separator">:</span>
                <span class="hc-neon-clock__digit hc-neon-clock__m0">0</span>
                <span class="hc-neon-clock__digit hc-neon-clock__m1">0</span>
            </div>
            <div class="hc-neon-clock__meta">
                <span class="hc-neon-clock__date">MON JAN 1</span>
                <span class="hc-neon-clock__seconds">00</span>
                <span class="hc-neon-clock__ampm">AM</span>
            </div>
        </div>`

    const query = (selector: string) => {
        const element = root.querySelector<HTMLElement>(selector)
        if (!element) throw new Error(`Neon Clock missing ${selector}`)
        return element
    }
    const timeEl = query('.hc-neon-clock__time')
    const separatorEl = query('.hc-neon-clock__separator')
    const metaEl = query('.hc-neon-clock__meta')
    const dateEl = query('.hc-neon-clock__date')
    const secondsEl = query('.hc-neon-clock__seconds')
    const ampmEl = query('.hc-neon-clock__ampm')
    const digits = [
        createDigitMorph(query('.hc-neon-clock__h0')),
        createDigitMorph(query('.hc-neon-clock__h1')),
        createDigitMorph(query('.hc-neon-clock__m0')),
        createDigitMorph(query('.hc-neon-clock__m1')),
    ]

    const drifters = makeDrifters(wide ? 36 : 22)
    const accentGlide = new SmoothedColor(palette.neonCyan)
    const secondaryGlide = new SmoothedColor(palette.electricPurple)
    let bootAt = Number.NaN
    let lastTime = Number.NaN

    return (time: number, controls: Record<string, unknown>) => {
        if (Number.isNaN(bootAt)) bootAt = time
        const boot = time - bootAt
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time
        const accent = accentGlide.update(controls.accent as string, dt)
        const secondary = secondaryGlide.update(controls.secondaryAccent as string, dt)
        const ink = resolveFaceInk(accent)
        const glow = clamp01((controls.glowIntensity as number) / 100)

        root.style.setProperty('--accent', accent)
        root.style.setProperty('--secondary', secondary)
        root.style.setProperty('--hero-ink', ink.hero)
        root.style.setProperty('--ui-ink', ink.ui)
        root.style.setProperty('--hero-font', `"${controls.headlineFont as string}", sans-serif`)
        root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
        // Monumental scale: the digits own the frame.
        const timeScale = (controls.timeSize as number) / 150
        const timeSize = wide ? ctx.height * 0.62 * timeScale : ctx.width * 0.31 * timeScale
        root.style.setProperty('--time-size', `${timeSize}`)
        root.style.setProperty('--digit-gap', `${(controls.digitSpacing as number) / 100}`)
        root.style.setProperty(
            '--meta-size',
            `${Math.max(9, (controls.metaSize as number) * (wide ? (ctx.height / 480) * 2.1 : ctx.width / 480))}`,
        )

        const now = new Date()
        const is12h = controls.hourFormat === '12h'
        let hours = now.getHours()
        const ampm = hours >= 12 ? 'PM' : 'AM'
        if (is12h) hours = hours % 12 || 12
        const hourText = is12h ? hours.toString() : hours.toString().padStart(2, '0')
        const minuteText = now.getMinutes().toString().padStart(2, '0')
        const secondsExact = now.getSeconds() + now.getMilliseconds() / 1000

        digits[0]?.(hourText.length > 1 ? (hourText[0] ?? '0') : '0', hourText.length < 2, time)
        digits[1]?.(hourText[hourText.length - 1] ?? '0', false, time)
        digits[2]?.(minuteText[0] ?? '0', false, time)
        digits[3]?.(minuteText[1] ?? '0', false, time)
        // The separator breathes once a second instead of hard-blinking.
        separatorEl.style.opacity = `${0.35 + 0.65 * (0.5 + 0.5 * Math.cos(secondsExact * Math.PI * 2))}`
        dateEl.textContent = now
            .toLocaleDateString('en-US', { day: 'numeric', month: 'short', weekday: 'short' })
            .toUpperCase()
        secondsEl.textContent = now.getSeconds().toString().padStart(2, '0')
        ampmEl.textContent = ampm

        const showTime = controls.showTime === true
        timeEl.classList.toggle('hc-neon-clock__hidden', !showTime)
        separatorEl.classList.toggle('hc-neon-clock__hidden', controls.showSeparator !== true)
        dateEl.classList.toggle('hc-neon-clock__hidden', controls.showDate !== true)
        secondsEl.classList.toggle('hc-neon-clock__hidden', controls.showSeconds !== true)
        ampmEl.classList.toggle('hc-neon-clock__hidden', !is12h || controls.showAmPm !== true)
        const metaVisible =
            controls.showDate === true || controls.showSeconds === true || (is12h && controls.showAmPm === true)
        metaEl.classList.toggle('hc-neon-clock__hidden', !metaVisible)

        // Entrance choreography: digits land first, meta follows.
        const timeIn = entrance(boot, 0.1, 1.0)
        const metaIn = entrance(boot, 0.45, 0.9)
        timeEl.style.opacity = `${timeIn}`
        timeEl.style.transform = `translateY(${(1 - timeIn) * 18}px)`
        metaEl.style.opacity = `${metaIn}`
        metaEl.style.transform = `translateY(${(1 - metaIn) * 12}px)`

        // ── Atmosphere ──
        const c = ctx.ctx
        const W = ctx.width
        const H = ctx.height
        c.clearRect(0, 0, W, H)
        if (atmosphereVisible(controls)) {
            drawNebulaField(c, W, H, time, accent, secondary, 0.5 + glow * 0.9)
            drawRisingMotes(c, W, H, time, drifters, accent, glow, 0.45)
        }

        if (controls.showDial !== true) return
        const dialStyle = (controls.dialStyle as string) ?? 'Orbit'
        const secondAngle = (secondsExact / 60) * Math.PI * 2 - Math.PI / 2
        const dialIn = entrance(boot, 0.7, 1.1)
        if (dialIn <= 0.01) return

        if (wide) {
            const left = W * 0.04
            const right = W * 0.96
            const railY = H * 0.88
            if (dialStyle === 'Pulse') {
                // The strip counterpart of the breathing ring: no comet, just
                // a full-width line that swells once per breath.
                const breath = 0.5 + 0.5 * Math.sin(time * 0.9)
                c.strokeStyle = withAlpha(accent, (0.18 + 0.2 * breath) * glow * dialIn)
                c.lineWidth = 1.5 + breath * 1.5
                c.beginPath()
                c.moveTo(left, railY)
                c.lineTo(right, railY)
                c.stroke()
                return
            }
            if (dialStyle === 'Split') {
                const minuteProgress = (now.getMinutes() + secondsExact / 60) / 60
                c.strokeStyle = withAlpha(secondary, 0.4 * glow * dialIn)
                c.lineWidth = 2
                c.beginPath()
                c.moveTo(left, railY + 7)
                c.lineTo(left + (right - left) * minuteProgress, railY + 7)
                c.stroke()
            }
            drawCometRail(c, left, right, railY, secondsExact / 60, accent, glow * dialIn)
            return
        }

        const cx = W / 2
        const cy = H / 2
        const radius = Math.min(W, H) * 0.44 * (0.92 + dialIn * 0.08)
        if (dialStyle === 'Pulse') {
            const breath = 0.5 + 0.5 * Math.sin(time * 0.9)
            c.strokeStyle = withAlpha(accent, (0.18 + 0.2 * breath) * glow * dialIn)
            c.lineWidth = 1.5 + breath * 1.5
            c.beginPath()
            c.arc(cx, cy, radius, 0, Math.PI * 2)
            c.stroke()
            return
        }
        if (dialStyle === 'Split') {
            const minuteProgress = (now.getMinutes() + secondsExact / 60) / 60
            c.strokeStyle = withAlpha(secondary, 0.4 * glow * dialIn)
            c.lineWidth = 2
            c.beginPath()
            c.arc(cx, cy, radius - 8, -Math.PI / 2, -Math.PI / 2 + minuteProgress * Math.PI * 2)
            c.stroke()
        }
        c.strokeStyle = withAlpha(accent, 0.1 * dialIn)
        c.lineWidth = 1
        c.beginPath()
        c.arc(cx, cy, radius, 0, Math.PI * 2)
        c.stroke()
        drawCometRing(c, cx, cy, radius, secondAngle, accent, glow * dialIn)
    }
}
