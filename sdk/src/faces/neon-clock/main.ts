import {
    color,
    combo,
    face,
    font,
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
    resolveFaceCanvasWash,
    resolveFaceSurface,
} from '../shared/dom'

const STYLE_ID = 'hc-face-neon-clock'

const STYLES = `
.hc-neon-clock {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --accent-glow: rgba(128, 255, 234, 0.36);
    --headline-font: 'Orbitron', sans-serif;
    --ui-font: 'Sora', sans-serif;
    --panel: transparent;
    --panel-edge: rgba(255, 255, 255, 0.08);
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: ${palette.fg.primary};
}

.hc-neon-clock__backdrop {
    position: absolute;
    inset: 16px;
    border-radius: 32px;
    border: 1px solid transparent;
    background: transparent;
    box-shadow: none;
}

.hc-neon-clock[data-panel='on'] .hc-neon-clock__backdrop {
    border-color: var(--panel-edge);
    background:
        radial-gradient(circle at 20% 20%, rgba(255,255,255,0.08), transparent 32%),
        linear-gradient(160deg, rgba(255,255,255,0.06), transparent 55%),
        var(--panel);
    box-shadow:
        inset 0 1px 0 rgba(255,255,255,0.06),
        0 24px 60px rgba(0, 0, 0, 0.42);
}

.hc-neon-clock[data-panel='on'][data-backdrop='clear'] .hc-neon-clock__backdrop {
    background:
        linear-gradient(180deg, rgba(255,255,255,0.04), rgba(255,255,255,0.02)),
        var(--panel);
    border-color: rgba(255,255,255,0.04);
    box-shadow: none;
}

.hc-neon-clock__stage {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 30px;
}

.hc-neon-clock__time-block {
    display: grid;
    gap: 14px;
    justify-items: center;
    text-align: center;
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
    text-shadow: 0 0 36px rgba(0, 0, 0, 0.36);
}

.hc-neon-clock__hours,
.hc-neon-clock__minutes {
    font-size: 108px;
}

.hc-neon-clock__seconds {
    font-size: 64px;
    align-self: center;
    opacity: 0.72;
}

.hc-neon-clock__separator {
    font-size: 88px;
    opacity: 0.6;
    transform: translateY(-4px);
}

.hc-neon-clock__meta {
    display: flex;
    gap: 18px;
    justify-content: center;
    font-family: var(--ui-font);
    font-size: 13px;
    letter-spacing: 0.24em;
    text-transform: uppercase;
    color: rgba(232, 230, 240, 0.6);
}
`

export default face(
    'Neon Clock',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        secondaryAccent: color('Secondary', palette.electricPurple, { group: 'Style' }),
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
                    headlineFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                    glowIntensity: 88,
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
                <div class="hc-neon-clock__time-block">
                    <div class="hc-neon-clock__time">
                        <span class="hc-neon-clock__hours">00</span>
                        <span class="hc-neon-clock__separator">:</span>
                        <span class="hc-neon-clock__minutes">00</span>
                        <span class="hc-neon-clock__seconds">00</span>
                    </div>
                    <div class="hc-neon-clock__meta">
                        <span class="hc-neon-clock__date">MON</span>
                        <span class="hc-neon-clock__ampm">24H</span>
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

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (_time, controls) => {
            const accent = controls.accent as string
            const secondary = controls.secondaryAccent as string
            const panelColor = controls.panelColor as string
            const panelAlpha = controls.panelAlpha as number
            const backdrop = controls.backdrop as string
            const glow = clamp01((controls.glowIntensity as number) / 100)
            const showSeconds = controls.showSeconds as boolean
            const showDate = controls.showDate as boolean
            const is12h = controls.hourFormat === '12h'

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.panel = panelAlpha > 0 ? 'on' : 'off'
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--accent-glow', withAlpha(accent, 0.18 + glow * 0.28))
            root.style.setProperty('--headline-font', `"${controls.headlineFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--panel', resolveFaceSurface(backdrop, panelColor, panelAlpha))

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

            const wash = resolveFaceCanvasWash(backdrop, panelColor, panelAlpha)
            if (wash) {
                c.fillStyle = wash
                c.fillRect(0, 0, W, H)
            }

            const bloom = c.createRadialGradient(cx, cy, 10, cx, cy, W * 0.46)
            bloom.addColorStop(0, withAlpha(accent, 0.12 + glow * 0.12))
            bloom.addColorStop(0.5, withAlpha(secondary, 0.06 + glow * 0.06))
            bloom.addColorStop(1, 'rgba(0,0,0,0)')
            c.fillStyle = bloom
            c.fillRect(0, 0, W, H)
        }
    },
)
