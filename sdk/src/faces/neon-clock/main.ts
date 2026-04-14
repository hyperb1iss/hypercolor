import {
    color,
    combo,
    face,
    font,
    num,
    palette,
    toggle,
    withAlpha,
    withGlow,
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
    --panel: rgba(10, 10, 18, 0.78);
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

.hc-neon-clock__chrome {
    position: absolute;
    inset: 0;
    padding: 30px;
}

.hc-neon-clock__topline,
.hc-neon-clock__footer {
    position: absolute;
    left: 30px;
    right: 30px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    font-family: var(--ui-font);
    font-size: 12px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: rgba(232, 230, 240, 0.72);
}

.hc-neon-clock__topline {
    top: 30px;
}

.hc-neon-clock__footer {
    bottom: 30px;
}

.hc-neon-clock__badge {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-radius: 999px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(10, 10, 18, 0.46);
    backdrop-filter: blur(16px);
}

.hc-neon-clock__badge-dot {
    width: 7px;
    height: 7px;
    border-radius: 999px;
    background: var(--accent);
    box-shadow: 0 0 18px var(--accent);
    animation: hcNeonClockBlink 1.4s ease-in-out infinite;
}

.hc-neon-clock__main {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
}

.hc-neon-clock__time-block {
    position: relative;
    display: grid;
    gap: 10px;
    justify-items: center;
    text-align: center;
}

.hc-neon-clock__orbit {
    position: absolute;
    inset: 50%;
    width: 72%;
    height: 72%;
    transform: translate(-50%, -50%);
    border-radius: 999px;
    border: 1px solid rgba(255,255,255,0.06);
    box-shadow:
        inset 0 0 24px rgba(255,255,255,0.04),
        0 0 28px rgba(0,0,0,0.36);
}

.hc-neon-clock[data-motion='minimal'] .hc-neon-clock__orbit {
    opacity: 0.35;
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
    font-size: 102px;
}

.hc-neon-clock__separator {
    font-size: 84px;
    opacity: 0.72;
    transform: translateY(-4px);
    animation: hcNeonClockBlink 1s steps(2) infinite;
}

.hc-neon-clock__seconds {
    min-width: 2.5ch;
    padding: 10px 12px 8px;
    border-radius: 18px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(255,255,255,0.04);
    font-family: var(--ui-font);
    font-size: 28px;
    font-weight: 600;
    letter-spacing: 0.12em;
}

.hc-neon-clock__meta {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    justify-content: center;
    font-family: var(--ui-font);
    font-size: 12px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: rgba(232, 230, 240, 0.72);
}

.hc-neon-clock__meta-pill {
    padding: 8px 12px;
    border-radius: 999px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(10,10,18,0.42);
    backdrop-filter: blur(14px);
}

.hc-neon-clock__progress {
    display: grid;
    gap: 10px;
    width: 100%;
}

.hc-neon-clock__meter {
    display: grid;
    gap: 6px;
}

.hc-neon-clock__meter-label {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-family: var(--ui-font);
    font-size: 11px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.66);
}

.hc-neon-clock__meter-rail {
    position: relative;
    height: 8px;
    border-radius: 999px;
    overflow: hidden;
    background: rgba(255,255,255,0.06);
}

.hc-neon-clock__meter-fill {
    position: absolute;
    inset: 0 auto 0 0;
    width: calc(var(--fill, 0) * 100%);
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), var(--secondary));
    box-shadow: 0 0 20px var(--accent-glow);
}

@keyframes hcNeonClockBlink {
    0%, 42%, 100% { opacity: 1; }
    50%, 92% { opacity: 0.35; }
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
        hourFormat: combo('Format', ['24h', '12h'], { group: 'Clock' }),
        showSeconds: toggle('Show Seconds', true, { group: 'Clock' }),
        showDate: toggle('Show Date', true, { group: 'Clock' }),
        motion: combo('Motion', ['Orbit', 'Bloom', 'Minimal'], { group: 'Style' }),
        backdrop: combo('Backdrop', ['Clear', 'Glass', 'Opaque'], { group: 'Style' }),
        glowIntensity: num('Glow', [0, 100], 78, { group: 'Style' }),
    },
    {
        description: 'A cinematic neon clock with glass chrome, animated orbits, luxe typography, and presets that actually change the mood.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'Electric Midnight',
                description: 'Orbitron over deep cyan and violet glass.',
                controls: {
                    accent: palette.neonCyan,
                    secondaryAccent: palette.electricPurple,
                    headlineFont: 'Orbitron',
                    uiFont: 'Sora',
                    motion: 'Orbit',
                    backdrop: 'Glass',
                    panelAlpha: 72,
                    glowIntensity: 84,
                },
            },
            {
                name: 'Blush Circuit',
                description: 'High-femme coral and purple with soft bloom.',
                controls: {
                    accent: palette.coral,
                    secondaryAccent: '#ffb3f2',
                    headlineFont: 'Audiowide',
                    uiFont: 'DM Sans',
                    motion: 'Bloom',
                    backdrop: 'Glass',
                    panelAlpha: 72,
                    glowIntensity: 70,
                },
            },
            {
                name: 'Arcade Mono',
                description: 'Monospaced synth clock with bright rails.',
                controls: {
                    accent: palette.electricYellow,
                    secondaryAccent: '#ff8d4d',
                    headlineFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                    motion: 'Orbit',
                    backdrop: 'Opaque',
                    panelAlpha: 92,
                    glowIntensity: 64,
                },
            },
            {
                name: 'Afterglow',
                description: 'Warm rose-gold chrome with restrained motion.',
                controls: {
                    accent: '#ffb38a',
                    secondaryAccent: '#ffd2c3',
                    headlineFont: 'Bebas Neue',
                    uiFont: 'Roboto Condensed',
                    motion: 'Minimal',
                    backdrop: 'Glass',
                    panelAlpha: 72,
                    glowIntensity: 58,
                },
            },
            {
                name: 'Frostline',
                description: 'Cool, airy blue-white with clear paneling.',
                controls: {
                    accent: '#9ae7ff',
                    secondaryAccent: '#d6ecff',
                    headlineFont: 'Exo 2',
                    uiFont: 'Inter',
                    motion: 'Bloom',
                    backdrop: 'Clear',
                    panelAlpha: 24,
                    glowIntensity: 62,
                },
            },
            {
                name: 'Night Drive',
                description: 'Cyberpunk magenta with bold condensed numerals.',
                controls: {
                    accent: '#ff4da6',
                    secondaryAccent: '#6a8bff',
                    headlineFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                    motion: 'Orbit',
                    backdrop: 'Opaque',
                    panelAlpha: 92,
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
            <div class="hc-neon-clock__chrome">
                <div class="hc-neon-clock__topline">
                    <div class="hc-neon-clock__badge">
                        <span class="hc-neon-clock__badge-dot"></span>
                        <span class="hc-neon-clock__mode">LOCAL TIME</span>
                    </div>
                    <div class="hc-neon-clock__badge hc-neon-clock__status">DISPLAY FACE</div>
                </div>
                <div class="hc-neon-clock__main">
                    <div class="hc-neon-clock__orbit"></div>
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
                <div class="hc-neon-clock__footer">
                    <div class="hc-neon-clock__progress">
                        <div class="hc-neon-clock__meter">
                            <div class="hc-neon-clock__meter-label"><span>SECOND SWEEP</span><span class="hc-neon-clock__second-label">00</span></div>
                            <div class="hc-neon-clock__meter-rail"><div class="hc-neon-clock__meter-fill hc-neon-clock__second-fill"></div></div>
                        </div>
                        <div class="hc-neon-clock__meter">
                            <div class="hc-neon-clock__meter-label"><span>DAYLIGHT</span><span class="hc-neon-clock__day-label">00%</span></div>
                            <div class="hc-neon-clock__meter-rail"><div class="hc-neon-clock__meter-fill hc-neon-clock__day-fill"></div></div>
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
        const secondLabelEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__second-label')!
        const dayLabelEl = root.querySelector<HTMLSpanElement>('.hc-neon-clock__day-label')!
        const secondFillEl = root.querySelector<HTMLDivElement>('.hc-neon-clock__second-fill')!
        const dayFillEl = root.querySelector<HTMLDivElement>('.hc-neon-clock__day-fill')!

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (time, controls) => {
            const accent = controls.accent as string
            const secondary = controls.secondaryAccent as string
            const glow = clamp01((controls.glowIntensity as number) / 100)
            const backdrop = controls.backdrop as string
            const panelColor = controls.panelColor as string
            const panelAlpha = controls.panelAlpha as number
            const motion = (controls.motion as string).toLowerCase()
            const showSeconds = controls.showSeconds as boolean
            const showDate = controls.showDate as boolean
            const is12h = controls.hourFormat === '12h'

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.panel = panelAlpha > 0 ? 'on' : 'off'
            root.dataset.motion = motion
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--accent-glow', withAlpha(accent, 0.18 + glow * 0.28))
            root.style.setProperty('--headline-font', `"${controls.headlineFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty(
                '--panel',
                resolveFaceSurface(backdrop, panelColor, panelAlpha),
            )

            const now = new Date()
            let hours = now.getHours()
            const minutes = now.getMinutes()
            const seconds = now.getSeconds()
            const milliseconds = now.getMilliseconds()
            const ampm = hours >= 12 ? 'PM' : 'AM'
            if (is12h) hours = hours % 12 || 12

            hoursEl.textContent = hours.toString().padStart(2, '0')
            minutesEl.textContent = minutes.toString().padStart(2, '0')
            secondsEl.textContent = seconds.toString().padStart(2, '0')
            secondsEl.style.display = showSeconds ? 'inline-flex' : 'none'
            dateEl.textContent = showDate
                ? now
                      .toLocaleDateString('en-US', {
                          weekday: 'short',
                          month: 'short',
                          day: 'numeric',
                      })
                      .toUpperCase()
                : 'SIGNAL FLOW'
            ampmEl.textContent = is12h ? ampm : '24H'

            const secondProgress = (seconds + milliseconds / 1000) / 60
            const dayProgress =
                (now.getHours() * 3600 + minutes * 60 + seconds + milliseconds / 1000) / 86400
            secondLabelEl.textContent = `${Math.round(secondProgress * 100)}%`
            dayLabelEl.textContent = `${Math.round(dayProgress * 100)}%`
            secondFillEl.style.setProperty('--fill', secondProgress.toFixed(4))
            dayFillEl.style.setProperty('--fill', dayProgress.toFixed(4))

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)
            const canvasWash = resolveFaceCanvasWash(backdrop, panelColor, panelAlpha)
            if (canvasWash) {
                c.fillStyle = canvasWash
                c.fillRect(0, 0, W, H)
            }

            const bloom = c.createRadialGradient(cx, cy, 10, cx, cy, W * 0.46)
            bloom.addColorStop(0, withAlpha(accent, 0.14 + glow * 0.14))
            bloom.addColorStop(0.45, withAlpha(secondary, 0.08 + glow * 0.08))
            bloom.addColorStop(1, 'rgba(0,0,0,0)')
            c.fillStyle = bloom
            c.fillRect(0, 0, W, H)

            const beam = c.createLinearGradient(0, H * 0.18, W, H * 0.84)
            beam.addColorStop(0, withAlpha(accent, 0))
            beam.addColorStop(0.48, withAlpha(accent, 0.12 + glow * 0.1))
            beam.addColorStop(0.52, withAlpha(secondary, 0.18 + glow * 0.1))
            beam.addColorStop(1, withAlpha(secondary, 0))
            c.fillStyle = beam
            c.fillRect(0, 0, W, H)

            const orbitRadius = Math.min(W, H) * 0.31
            c.lineWidth = 3
            c.lineCap = 'round'
            c.strokeStyle = withAlpha(accent, 0.12)
            c.beginPath()
            c.arc(cx, cy, orbitRadius, 0, Math.PI * 2)
            c.stroke()

            const sweepAngle = -Math.PI / 2 + secondProgress * Math.PI * 2
            if (motion !== 'minimal') {
                withGlow(c, accent, glow, () => {
                    c.strokeStyle = accent
                    c.beginPath()
                    c.arc(cx, cy, orbitRadius, -Math.PI / 2, sweepAngle)
                    c.stroke()
                })

                const dotX = cx + Math.cos(sweepAngle) * orbitRadius
                const dotY = cy + Math.sin(sweepAngle) * orbitRadius
                withGlow(c, secondary, glow * 1.2, () => {
                    c.fillStyle = secondary
                    c.beginPath()
                    c.arc(dotX, dotY, 5 + glow * 3, 0, Math.PI * 2)
                    c.fill()
                })
            }

            if (motion === 'bloom') {
                const petals = 6
                for (let i = 0; i < petals; i++) {
                    const angle = time * 0.55 + (Math.PI * 2 * i) / petals
                    const x = cx + Math.cos(angle) * orbitRadius * 0.5
                    const y = cy + Math.sin(angle) * orbitRadius * 0.5
                    const orb = c.createRadialGradient(x, y, 2, x, y, 44)
                    orb.addColorStop(0, withAlpha(secondary, 0.16))
                    orb.addColorStop(1, withAlpha(secondary, 0))
                    c.fillStyle = orb
                    c.fillRect(x - 44, y - 44, 88, 88)
                }
            }

            c.strokeStyle = withAlpha('#ffffff', 0.04)
            c.lineWidth = 1
            for (let i = -1; i <= 1; i++) {
                const y = H * 0.22 + i * 10 + Math.sin(time * 0.8 + i) * 4
                c.beginPath()
                c.moveTo(32, y)
                c.lineTo(W - 32, y)
                c.stroke()
            }
        }
    },
)
