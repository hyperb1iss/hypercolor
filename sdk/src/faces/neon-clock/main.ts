import {
    arcGauge,
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
    resolveFaceSurface,
} from '../shared/dom'

const STYLE_ID = 'hc-face-neon-clock'

const STYLES = `
.hc-neon-clock {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --headline-font: 'Orbitron', sans-serif;
    --ui-font: 'Sora', sans-serif;
    --panel: transparent;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    --panel-edge: rgba(255, 255, 255, 0.08);
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
}

.hc-neon-clock__backdrop {
    display: none;
}

.hc-neon-clock__stage {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 34px;
}

.hc-neon-clock__stack {
    display: grid;
    gap: 16px;
    justify-items: center;
    text-align: center;
}

.hc-neon-clock__eyebrow {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    flex-wrap: wrap;
}

.hc-neon-clock__chip {
    padding: 8px 14px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--accent) 16%, rgba(255,255,255,0.1));
    background: rgba(7, 8, 14, 0.24);
    font-family: var(--ui-font);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.22em;
    text-transform: uppercase;
    color: var(--ui-ink);
    backdrop-filter: blur(8px);
}

.hc-neon-clock__chip--accent {
    color: var(--hero-ink);
    border-color: color-mix(in srgb, var(--accent) 42%, transparent);
    box-shadow: 0 0 18px color-mix(in srgb, var(--accent) 16%, transparent);
}

.hc-neon-clock__time-block {
    display: grid;
    gap: 14px;
    justify-items: center;
}

.hc-neon-clock__time {
    display: flex;
    align-items: baseline;
    gap: 12px;
    font-family: var(--headline-font);
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    line-height: 0.9;
    color: var(--hero-ink);
    text-shadow:
        0 0 20px color-mix(in srgb, var(--accent) 14%, transparent),
        0 8px 30px rgba(0, 0, 0, 0.34);
}

.hc-neon-clock__hours,
.hc-neon-clock__minutes {
    font-size: 104px;
}

.hc-neon-clock__seconds {
    font-size: 54px;
    align-self: center;
    color: var(--ui-ink);
}

.hc-neon-clock__separator {
    font-size: 88px;
    color: var(--dim-ink);
    transform: translateY(-4px);
}

.hc-neon-clock__meta {
    display: flex;
    gap: 12px;
    justify-content: center;
    flex-wrap: wrap;
}

.hc-neon-clock__meta-pill {
    padding: 7px 12px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--accent) 10%, rgba(255,255,255,0.08));
    background: rgba(7, 8, 14, 0.2);
    font-family: var(--ui-font);
    font-size: 11px;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-neon-clock[data-style='split'] .hc-neon-clock__time {
    letter-spacing: 0.12em;
}
`

export default face(
    'Neon Clock',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        secondaryAccent: color('Secondary', palette.electricPurple, { group: 'Style' }),
        dialStyle: combo('Dial Style', ['Orbit', 'Split', 'Pulse'], { group: 'Clock' }),
        headlineFont: font('Headline Font', 'Orbitron', { group: 'Typography', families: [...DISPLAY_FONT_FAMILIES] }),
        uiFont: font('UI Font', 'Sora', { group: 'Typography', families: [...UI_FONT_FAMILIES] }),
        panelColor: color('Panel Color', palette.bg.deep, { group: 'Style' }),
        panelAlpha: num('Panel Alpha', [0, 100], 0, { group: 'Style' }),
        backdrop: combo('Backdrop', ['Clear', 'Glass', 'Opaque'], { group: 'Style' }),
        hourFormat: combo('Format', ['24h', '12h'], { group: 'Clock' }),
        showSeconds: toggle('Show Seconds', true, { group: 'Clock' }),
        showDate: toggle('Show Date', true, { group: 'Clock' }),
        glowIntensity: num('Glow', [0, 100], 78, { group: 'Style' }),
    },
    {
        description: 'A cinematic neon clock with luxe typography, presets that actually change the mood, and nothing between you and the time.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'Electric Midnight',
                description: 'Orbitron over deep cyan and violet glow.',
                controls: {
                    accent: palette.neonCyan,
                    secondaryAccent: palette.electricPurple,
                    dialStyle: 'Orbit',
                    headlineFont: 'Orbitron',
                    uiFont: 'Sora',
                    glowIntensity: 84,
                },
            },
            {
                name: 'Blush Circuit',
                description: 'High-femme coral and purple bloom.',
                controls: {
                    accent: palette.coral,
                    secondaryAccent: '#ffb3f2',
                    dialStyle: 'Pulse',
                    headlineFont: 'Audiowide',
                    uiFont: 'DM Sans',
                    glowIntensity: 70,
                },
            },
            {
                name: 'Arcade Mono',
                description: 'Monospaced synth clock with bright accents.',
                controls: {
                    accent: palette.electricYellow,
                    secondaryAccent: '#ff8d4d',
                    dialStyle: 'Split',
                    headlineFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                    glowIntensity: 64,
                },
            },
            {
                name: 'Afterglow',
                description: 'Warm rose-gold with restrained glow.',
                controls: {
                    accent: '#ffb38a',
                    secondaryAccent: '#ffd2c3',
                    dialStyle: 'Pulse',
                    headlineFont: 'Bebas Neue',
                    uiFont: 'Roboto Condensed',
                    glowIntensity: 58,
                },
            },
            {
                name: 'Frostline',
                description: 'Cool, airy blue-white.',
                controls: {
                    accent: '#9ae7ff',
                    secondaryAccent: '#d6ecff',
                    dialStyle: 'Orbit',
                    headlineFont: 'Exo 2',
                    uiFont: 'Inter',
                    glowIntensity: 62,
                },
            },
            {
                name: 'Night Drive',
                description: 'Cyberpunk magenta with bold numerals.',
                controls: {
                    accent: '#ff4da6',
                    secondaryAccent: '#6a8bff',
                    dialStyle: 'Split',
                    headlineFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                    glowIntensity: 88,
                },
            },
            {
                name: 'Signal Drift',
                description: 'Cyan and amber orbit with softer UI meta.',
                controls: {
                    accent: '#8ef4ff',
                    secondaryAccent: '#ffc76b',
                    dialStyle: 'Orbit',
                    headlineFont: 'Orbitron',
                    uiFont: 'DM Sans',
                    glowIntensity: 72,
                },
            },
            {
                name: 'Prism Pulse',
                description: 'Bright pulse ring with femme magenta glow.',
                controls: {
                    accent: '#ff78d2',
                    secondaryAccent: '#8fcbff',
                    dialStyle: 'Pulse',
                    headlineFont: 'Audiowide',
                    uiFont: 'Space Grotesk',
                    glowIntensity: 82,
                },
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-neon-clock')
        root.innerHTML = `
            <div class="hc-neon-clock__backdrop"></div>
            <div class="hc-neon-clock__stage">
                <div class="hc-neon-clock__stack">
                    <div class="hc-neon-clock__eyebrow">
                        <div class="hc-neon-clock__chip hc-neon-clock__chip--accent hc-neon-clock__mode">ORBIT</div>
                        <div class="hc-neon-clock__chip">LOCAL</div>
                    </div>
                    <div class="hc-neon-clock__time-block">
                        <div class="hc-neon-clock__time">
                            <span class="hc-neon-clock__hours">00</span>
                            <span class="hc-neon-clock__separator">:</span>
                            <span class="hc-neon-clock__minutes">00</span>
                            <span class="hc-neon-clock__seconds">00</span>
                        </div>
                        <div class="hc-neon-clock__meta">
                            <span class="hc-neon-clock__meta-pill hc-neon-clock__date">MON</span>
                            <span class="hc-neon-clock__meta-pill hc-neon-clock__ampm">24H</span>
                        </div>
                    </div>
                </div>
            </div>
        `

        const hoursEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__hours')!
        const minutesEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__minutes')!
        const secondsEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__seconds')!
        const dateEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__date')!
        const ampmEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__ampm')!
        const metaEl = root.querySelector<HTMLDivElement>('.hc-neon-clock__meta')!
        const modeEl = root.querySelector<HTMLDivElement>('.hc-neon-clock__mode')!

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (_time, controls) => {
            const accent = lerpColor(controls.accent as string, palette.fg.primary, 0.06)
            const secondary = mixFaceAccent(controls.secondaryAccent as string, accent, 0.12)
            const ink = resolveFaceInk(accent)
            const panelColor = controls.panelColor as string
            const panelAlpha = controls.panelAlpha as number
            const backdrop = controls.backdrop as string
            const glow = clamp01((controls.glowIntensity as number) / 100)
            const showSeconds = controls.showSeconds as boolean
            const showDate = controls.showDate as boolean
            const is12h = controls.hourFormat === '12h'
            const dialStyle = (controls.dialStyle as string).toLowerCase()

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.panel = panelAlpha > 0 ? 'on' : 'off'
            root.dataset.style = dialStyle
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--hero-ink', ink.hero)
            root.style.setProperty('--ui-ink', ink.ui)
            root.style.setProperty('--dim-ink', ink.dim)
            root.style.setProperty('--panel-edge', ink.edge)
            root.style.setProperty('--headline-font', `"${controls.headlineFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--panel', resolveFaceSurface(backdrop, panelColor, panelAlpha))
            modeEl.textContent = dialStyle.toUpperCase()

            const now = new Date()
            let hours = now.getHours()
            const minutes = now.getMinutes()
            const seconds = now.getSeconds()
            const ampm = hours >= 12 ? 'PM' : 'AM'
            if (is12h) hours = hours % 12 || 12

            hoursEl.textContent = hours.toString().padStart(2, '0')
            minutesEl.textContent = minutes.toString().padStart(2, '0')
            secondsEl.textContent = seconds.toString().padStart(2, '0')
            secondsEl.style.display = showSeconds ? 'inline-flex' : 'none'

            if (showDate || is12h) {
                dateEl.textContent = showDate
                    ? now
                          .toLocaleDateString('en-US', {
                              weekday: 'short',
                              month: 'short',
                              day: 'numeric',
                          })
                          .toUpperCase()
                    : ''
                ampmEl.textContent = is12h ? ampm : ''
                dateEl.style.display = showDate ? 'inline' : 'none'
                ampmEl.style.display = is12h ? 'inline' : 'none'
                metaEl.style.display = 'flex'
            } else {
                metaEl.style.display = 'none'
            }

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)

            const progress = showSeconds ? seconds / 60 : minutes / 60
            const radius = dialStyle === 'split' ? 172 : dialStyle === 'pulse' ? 160 : 166
            const thickness = dialStyle === 'split' ? 12 : dialStyle === 'pulse' ? 18 : 14
            arcGauge(c, {
                cx,
                cy,
                radius,
                thickness,
                value: progress,
                fillColor: [accent, secondary],
                trackColor: withAlpha(ink.ui, 0.1),
                startAngle: Math.PI * 0.74,
                sweep: dialStyle === 'split' ? Math.PI * 1.16 : Math.PI * 1.66,
                glow: 0.28 + glow * 0.48,
            })

            for (let index = 0; index < 60; index++) {
                const angle = -Math.PI / 2 + (index / 60) * Math.PI * 2
                const active = index <= seconds
                const dotRadius = index % 5 === 0 ? 164 : 172
                const x = cx + Math.cos(angle) * dotRadius
                const y = cy + Math.sin(angle) * dotRadius
                c.fillStyle = active
                    ? withAlpha(index % 2 === 0 ? accent : secondary, 0.42 + glow * 0.18)
                    : withAlpha(ink.ui, 0.1)
                c.beginPath()
                c.arc(x, y, index % 5 === 0 ? 2.8 : 1.5, 0, Math.PI * 2)
                c.fill()
            }

            for (let index = 0; index < 12; index++) {
                const angle = -Math.PI / 2 + (index / 12) * Math.PI * 2 + _time * 0.02
                const x = cx + Math.cos(angle) * 116
                const y = cy + Math.sin(angle) * 116
                c.fillStyle = withAlpha(index % 3 === 0 ? secondary : accent, 0.08 + glow * 0.06)
                c.beginPath()
                c.arc(x, y, index % 3 === 0 ? 4.5 : 3, 0, Math.PI * 2)
                c.fill()
            }
        }
    },
)
