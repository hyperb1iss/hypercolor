import {
    color,
    combo,
    face,
    font,
    lerpColor,
    num,
    palette,
    toggle,
    withAlpha,
} from '@hypercolor/sdk'

import {
    DISPLAY_FONT_FAMILIES,
    UI_FONT_FAMILIES,
    clamp01,
    createFaceRoot,
    ensureFaceStyles,
    mixFaceAccent,
    resolveFaceInk,
} from '../shared/dom'

const STYLE_ID = 'hc-face-neon-clock'

const STYLES = `
.hc-neon-clock {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --headline-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
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
    transform: translate(-50%, -50%);
    display: inline-flex;
    align-items: baseline;
    justify-content: center;
    gap: 10px;
    font-family: var(--headline-font);
    font-size: 120px;
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
    display: inline-grid;
    grid-auto-flow: column;
    justify-content: center;
    grid-template-columns: repeat(2, 0.68ch);
}

.hc-neon-clock__digit {
    display: inline-flex;
    width: 0.68ch;
    justify-content: center;
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
    top: calc(50% + 88px);
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 14px;
    font-family: var(--ui-font);
    font-size: 12px;
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
`

function setClockDigit(slot: HTMLSpanElement, value: string | null): void {
    slot.textContent = value ?? '0'
    slot.classList.toggle('hc-neon-clock__digit--blank', value == null)
}

export default face(
    'Neon Clock',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        secondaryAccent: color('Secondary', palette.electricPurple, { group: 'Style' }),
        dialStyle: combo('Dial Style', ['Orbit', 'Split', 'Pulse'], { group: 'Clock' }),
        headlineFont: font('Headline Font', 'Rajdhani', { group: 'Typography', families: [...DISPLAY_FONT_FAMILIES] }),
        uiFont: font('UI Font', 'Inter', { group: 'Typography', families: [...UI_FONT_FAMILIES] }),
        hourFormat: combo('Format', ['24h', '12h'], { group: 'Clock' }),
        glowIntensity: num('Glow', [0, 100], 56, { group: 'Style' }),
        showTime: toggle('Show Time', true, { group: 'Elements' }),
        showSeparator: toggle('Show Separator', true, { group: 'Elements' }),
        showDate: toggle('Show Date', true, { group: 'Elements' }),
        showSeconds: toggle('Show Seconds', false, { group: 'Elements' }),
        showAmPm: toggle('Show AM/PM', true, { group: 'Elements' }),
        showDial: toggle('Show Dial', true, { group: 'Elements' }),
    },
    {
        description: 'A centered digital clock. Every element is independently toggleable.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'Electric Midnight',
                description: 'Cyan-violet clock with balanced tech numerals.',
                controls: {
                    accent: palette.neonCyan,
                    secondaryAccent: palette.electricPurple,
                    dialStyle: 'Orbit',
                    headlineFont: 'Rajdhani',
                    uiFont: 'Inter',
                    glowIntensity: 60,
                },
            },
            {
                name: 'Blush Circuit',
                description: 'Soft coral and purple with crisp modern type.',
                controls: {
                    accent: palette.coral,
                    secondaryAccent: '#ffb3f2',
                    dialStyle: 'Pulse',
                    headlineFont: 'Exo 2',
                    uiFont: 'DM Sans',
                    glowIntensity: 54,
                },
            },
            {
                name: 'Arcade Mono',
                description: 'Monospaced clock with clean amber accents.',
                controls: {
                    accent: palette.electricYellow,
                    secondaryAccent: '#ff8d4d',
                    dialStyle: 'Split',
                    headlineFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                    glowIntensity: 48,
                },
            },
            {
                name: 'Afterglow',
                description: 'Warm rose-gold with compact numerals.',
                controls: {
                    accent: '#ffb38a',
                    secondaryAccent: '#ffd2c3',
                    dialStyle: 'Pulse',
                    headlineFont: 'Rajdhani',
                    uiFont: 'Roboto Condensed',
                    glowIntensity: 42,
                },
            },
            {
                name: 'Frostline',
                description: 'Cool blue-white with airy accents.',
                controls: {
                    accent: '#9ae7ff',
                    secondaryAccent: '#d6ecff',
                    dialStyle: 'Orbit',
                    headlineFont: 'Exo 2',
                    uiFont: 'Inter',
                    glowIntensity: 44,
                },
            },
            {
                name: 'Night Drive',
                description: 'Magenta-blue contrast with strong readable rhythm.',
                controls: {
                    accent: '#ff4da6',
                    secondaryAccent: '#6a8bff',
                    dialStyle: 'Split',
                    headlineFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                    glowIntensity: 58,
                },
            },
            {
                name: 'Signal Drift',
                description: 'Cyan and amber with subtle orbit accent.',
                controls: {
                    accent: '#8ef4ff',
                    secondaryAccent: '#ffc76b',
                    dialStyle: 'Orbit',
                    headlineFont: 'Orbitron',
                    uiFont: 'DM Sans',
                    glowIntensity: 50,
                },
            },
            {
                name: 'Naked Time',
                description: 'Just the digits. No meta, no dial.',
                controls: {
                    accent: palette.neonCyan,
                    secondaryAccent: palette.electricPurple,
                    headlineFont: 'Rajdhani',
                    uiFont: 'Inter',
                    showDate: false,
                    showSeconds: false,
                    showAmPm: false,
                    showDial: false,
                },
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-neon-clock')
        root.innerHTML = `
            <div class="hc-neon-clock__time">
                <span class="hc-neon-clock__slot hc-neon-clock__slot--hours">
                    <span class="hc-neon-clock__digit hc-neon-clock__hours-tens">0</span>
                    <span class="hc-neon-clock__digit hc-neon-clock__hours-ones">0</span>
                </span>
                <span class="hc-neon-clock__separator">:</span>
                <span class="hc-neon-clock__slot hc-neon-clock__slot--minutes">
                    <span class="hc-neon-clock__digit hc-neon-clock__minutes-tens">0</span>
                    <span class="hc-neon-clock__digit hc-neon-clock__minutes-ones">0</span>
                </span>
            </div>
            <div class="hc-neon-clock__meta">
                <span class="hc-neon-clock__date"></span>
                <span class="hc-neon-clock__seconds"></span>
                <span class="hc-neon-clock__ampm"></span>
            </div>
        `

        const timeEl = root.querySelector<HTMLDivElement>('.hc-neon-clock__time')!
        const separatorEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__separator')!
        const hoursTensEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__hours-tens')!
        const hoursOnesEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__hours-ones')!
        const minutesTensEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__minutes-tens')!
        const minutesOnesEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__minutes-ones')!
        const secondsEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__seconds')!
        const dateEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__date')!
        const ampmEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__ampm')!
        const metaEl = root.querySelector<HTMLDivElement>('.hc-neon-clock__meta')!

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (_time, controls) => {
            const accent = lerpColor(controls.accent as string, palette.fg.primary, 0.06)
            const secondary = mixFaceAccent(controls.secondaryAccent as string, accent, 0.12)
            const ink = resolveFaceInk(accent)
            const glow = clamp01((controls.glowIntensity as number) / 100)
            const showTime = controls.showTime as boolean
            const showSeparator = controls.showSeparator as boolean
            const showSeconds = controls.showSeconds as boolean
            const showDate = controls.showDate as boolean
            const showAmPm = controls.showAmPm as boolean
            const showDial = controls.showDial as boolean
            const is12h = controls.hourFormat === '12h'
            const dialStyle = (controls.dialStyle as string).toLowerCase()

            root.dataset.style = dialStyle
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--hero-ink', ink.hero)
            root.style.setProperty('--ui-ink', ink.ui)
            root.style.setProperty('--dim-ink', ink.dim)
            root.style.setProperty('--headline-font', `"${controls.headlineFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)

            const now = new Date()
            let hours = now.getHours()
            const minutes = now.getMinutes()
            const seconds = now.getSeconds()
            const ampm = hours >= 12 ? 'PM' : 'AM'
            if (is12h) hours = hours % 12 || 12

            const hoursText = hours.toString()
            const minutesText = minutes.toString().padStart(2, '0')
            setClockDigit(hoursTensEl, hoursText.length > 1 ? hoursText[0] ?? null : null)
            setClockDigit(hoursOnesEl, hoursText[hoursText.length - 1] ?? '0')
            setClockDigit(minutesTensEl, minutesText[0] ?? '0')
            setClockDigit(minutesOnesEl, minutesText[1] ?? '0')
            secondsEl.textContent = `SEC ${seconds.toString().padStart(2, '0')}`
            ampmEl.textContent = is12h ? ampm : ''
            dateEl.textContent = now
                .toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric' })
                .toUpperCase()

            timeEl.classList.toggle('hc-neon-clock__hidden', !showTime)
            separatorEl.classList.toggle('hc-neon-clock__hidden', !showSeparator)
            dateEl.classList.toggle('hc-neon-clock__hidden', !showDate)
            secondsEl.classList.toggle('hc-neon-clock__hidden', !showSeconds)
            ampmEl.classList.toggle('hc-neon-clock__hidden', !is12h || !showAmPm)
            metaEl.classList.toggle(
                'hc-neon-clock__hidden',
                !showDate && !showSeconds && !(is12h && showAmPm),
            )

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)
            if (!showDial) return

            c.save()
            c.strokeStyle = withAlpha(accent, 0.16 + glow * 0.14)
            c.shadowColor = withAlpha(accent, 0.16 + glow * 0.12)
            c.shadowBlur = 20 + glow * 24

            if (dialStyle === 'split') {
                c.lineWidth = 3
                c.lineCap = 'round'
                c.beginPath()
                c.moveTo(cx - 112, cy + 78)
                c.lineTo(cx - 36, cy + 78)
                c.moveTo(cx + 36, cy + 78)
                c.lineTo(cx + 112, cy + 78)
                c.stroke()
            } else if (dialStyle === 'pulse') {
                c.lineWidth = 4
                c.lineCap = 'round'
                c.beginPath()
                c.moveTo(cx - 92, cy + 74)
                c.quadraticCurveTo(cx, cy + 94, cx + 92, cy + 74)
                c.stroke()
            } else {
                c.lineWidth = 5
                c.lineCap = 'round'
                c.beginPath()
                c.arc(cx, cy + 8, 122, Math.PI * 0.78, Math.PI * 1.28)
                c.stroke()
            }

            c.restore()
        }
    },
)
