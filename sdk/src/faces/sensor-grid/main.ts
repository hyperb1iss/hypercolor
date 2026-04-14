import {
    ValueHistory,
    color,
    colorByValue,
    combo,
    face,
    font,
    lerpColor,
    num,
    palette,
    sensor,
    sensorColors,
    sparkline,
    toggle,
    withAlpha,
} from '@hypercolor/sdk'

import {
    DISPLAY_FONT_FAMILIES,
    UI_FONT_FAMILIES,
    clamp01,
    createFaceRoot,
    ensureFaceStyles,
    humanizeSensorLabel,
    mixFaceAccent,
    resolveFaceInk,
    resolveFaceSurface,
} from '../shared/dom'

const STYLE_ID = 'hc-face-sensor-grid'

const STYLES = `
.hc-sensor-grid {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Sora', sans-serif;
    --panel: transparent;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    --edge-ink: rgba(255,255,255,0.12);
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
}

.hc-sensor-grid__panel {
    display: none;
}

.hc-sensor-grid__layout {
    position: absolute;
    inset: 0;
    padding: 34px;
}

.hc-sensor-grid__cards {
    width: 100%;
    height: 100%;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    grid-template-rows: repeat(2, minmax(0, 1fr));
    gap: 28px;
}

.hc-sensor-grid__card {
    display: grid;
    gap: 12px;
    align-content: stretch;
    padding: 18px 18px 16px;
    border-radius: 28px;
    border: 1px solid color-mix(in srgb, var(--accent) 10%, rgba(255,255,255,0.06));
    background:
        linear-gradient(180deg, rgba(9, 10, 18, 0.28), rgba(9, 10, 18, 0.08)),
        color-mix(in srgb, var(--accent) 6%, transparent);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.03);
}

.hc-sensor-grid__card-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
}

.hc-sensor-grid__chip {
    padding: 7px 12px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--accent) 18%, rgba(255,255,255,0.08));
    background: rgba(7, 8, 14, 0.24);
    font-family: var(--ui-font);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-sensor-grid__percent {
    font-family: var(--ui-font);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.22em;
    text-transform: uppercase;
    color: var(--dim-ink);
}

.hc-sensor-grid__value {
    font-family: var(--hero-font);
    font-size: 62px;
    font-weight: 700;
    line-height: 0.92;
    letter-spacing: 0.04em;
    color: var(--hero-ink);
    text-shadow:
        0 0 18px color-mix(in srgb, var(--accent) 14%, transparent),
        0 8px 24px rgba(0,0,0,0.26);
}

.hc-sensor-grid__label {
    font-family: var(--ui-font);
    font-size: 11px;
    letter-spacing: 0.22em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-sensor-grid__track {
    position: relative;
    width: 100%;
    height: 9px;
    border-radius: 999px;
    overflow: hidden;
    background: rgba(255,255,255,0.05);
    margin-top: auto;
}

.hc-sensor-grid__track-fill {
    position: absolute;
    inset: 0 auto 0 0;
    width: calc(var(--fill, 0) * 100%);
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), var(--secondary));
}

.hc-sensor-grid[data-style='radar'] .hc-sensor-grid__card {
    border-radius: 30px 18px 30px 18px;
}

.hc-sensor-grid[data-style='signal'] .hc-sensor-grid__value {
    letter-spacing: 0.08em;
}
`

export default face(
    'Sensor Grid',
    {
        sensor1: sensor('Top Left', 'cpu_temp', { group: 'Sensors' }),
        sensor2: sensor('Top Right', 'gpu_temp', { group: 'Sensors' }),
        sensor3: sensor('Bottom Left', 'cpu_load', { group: 'Sensors' }),
        sensor4: sensor('Bottom Right', 'ram_used', { group: 'Sensors' }),
        colorMode: combo('Colors', ['Auto', 'Accent'], { group: 'Style' }),
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        frameStyle: combo('Frame Style', ['Atlas', 'Signal', 'Radar'], { group: 'Layout' }),
        heroFont: font('Hero Font', 'Rajdhani', { group: 'Typography', families: [...DISPLAY_FONT_FAMILIES] }),
        uiFont: font('UI Font', 'Sora', { group: 'Typography', families: [...UI_FONT_FAMILIES] }),
        panelColor: color('Panel Color', palette.bg.deep, { group: 'Style' }),
        panelAlpha: num('Panel Alpha', [0, 100], 0, { group: 'Style' }),
        backdrop: combo('Backdrop', ['Clear', 'Glass', 'Opaque'], { group: 'Style' }),
        showTracks: toggle('Tracks', true, { group: 'Style' }),
    },
    {
        description: 'A modular four-panel dashboard with rich typography and presets for vitals, thermals, load, and memory snapshots.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'System Vitals',
                description: 'Balanced cyan dashboard for CPU, GPU, load, and memory.',
                controls: {
                    sensor1: 'cpu_temp',
                    sensor2: 'gpu_temp',
                    sensor3: 'cpu_load',
                    sensor4: 'ram_used',
                    colorMode: 'Auto',
                    frameStyle: 'Atlas',
                    heroFont: 'Rajdhani',
                    uiFont: 'Sora',
                },
            },
            {
                name: 'Thermal Club',
                description: 'All-temperature layout with condensed numerals.',
                controls: {
                    sensor1: 'cpu_temp',
                    sensor2: 'gpu_temp',
                    sensor3: 'cpu_temp',
                    sensor4: 'gpu_temp',
                    colorMode: 'Auto',
                    frameStyle: 'Signal',
                    heroFont: 'Bebas Neue',
                    uiFont: 'Roboto Condensed',
                },
            },
            {
                name: 'Arctic Rail',
                description: 'Cool blue accent with airy type.',
                controls: {
                    colorMode: 'Accent',
                    accent: '#9ae7ff',
                    frameStyle: 'Atlas',
                    heroFont: 'Exo 2',
                    uiFont: 'Inter',
                },
            },
            {
                name: 'Signal Pink',
                description: 'Femme coral matrix with bold display font.',
                controls: {
                    colorMode: 'Accent',
                    accent: palette.coral,
                    frameStyle: 'Radar',
                    heroFont: 'Audiowide',
                    uiFont: 'DM Sans',
                },
            },
            {
                name: 'Mono Ops',
                description: 'Sharp monospaced cards for clean telemetry.',
                controls: {
                    colorMode: 'Auto',
                    frameStyle: 'Signal',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                },
            },
            {
                name: 'Gold Deck',
                description: 'Warm gold accent.',
                controls: {
                    colorMode: 'Accent',
                    accent: palette.electricYellow,
                    frameStyle: 'Radar',
                    heroFont: 'Orbitron',
                    uiFont: 'Space Grotesk',
                },
            },
            {
                name: 'Night Mesh',
                description: 'Blue-magenta telemetry mesh with sharper card rails.',
                controls: {
                    colorMode: 'Accent',
                    accent: '#8ed8ff',
                    frameStyle: 'Signal',
                    heroFont: 'Orbitron',
                    uiFont: 'DM Sans',
                },
            },
            {
                name: 'Amber Atlas',
                description: 'Warm amber survey deck with clear card hierarchy.',
                controls: {
                    colorMode: 'Accent',
                    accent: '#ffb45f',
                    frameStyle: 'Atlas',
                    heroFont: 'Bebas Neue',
                    uiFont: 'Space Grotesk',
                },
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-sensor-grid')
        root.innerHTML = `
            <div class="hc-sensor-grid__panel"></div>
            <div class="hc-sensor-grid__layout">
                <div class="hc-sensor-grid__cards">
                    ${Array.from({ length: 4 }, (_, index) => `
                        <div class="hc-sensor-grid__card" data-card="${index}">
                            <div class="hc-sensor-grid__card-head">
                                <div class="hc-sensor-grid__chip">LIVE</div>
                                <div class="hc-sensor-grid__percent">0%</div>
                            </div>
                            <div class="hc-sensor-grid__value">--</div>
                            <div class="hc-sensor-grid__label">UNASSIGNED</div>
                            <div class="hc-sensor-grid__track"><div class="hc-sensor-grid__track-fill"></div></div>
                        </div>
                    `).join('')}
                </div>
            </div>
        `

        const cards = Array.from(root.querySelectorAll<HTMLDivElement>('.hc-sensor-grid__card'))
        const sensorKeys = ['sensor1', 'sensor2', 'sensor3', 'sensor4'] as const
        const smoothValues = [0, 0, 0, 0]
        const histories = Array.from({ length: 4 }, () => new ValueHistory(36))
        let lastHistoryPush = 0

        const { width: W, height: H } = ctx

        return (time, controls, sensors) => {
            const colorMode = controls.colorMode as string
            const accent = lerpColor(controls.accent as string, palette.fg.primary, 0.04)
            const secondary = mixFaceAccent(accent)
            const ink = resolveFaceInk(accent)
            const panelColor = controls.panelColor as string
            const panelAlpha = controls.panelAlpha as number
            const backdrop = controls.backdrop as string
            const frameStyle = (controls.frameStyle as string).toLowerCase()

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.panel = panelAlpha > 0 ? 'on' : 'off'
            root.dataset.style = frameStyle
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--hero-ink', ink.hero)
            root.style.setProperty('--ui-ink', ink.ui)
            root.style.setProperty('--dim-ink', ink.dim)
            root.style.setProperty('--edge-ink', ink.edge)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--panel', resolveFaceSurface(backdrop, panelColor, panelAlpha))

            if (time - lastHistoryPush > 0.18) {
                lastHistoryPush = time
                sensorKeys.forEach((key, index) => histories[index].push(sensors.normalized(controls[key] as string)))
            }

            cards.forEach((card, index) => {
                const sensorLabel = controls[sensorKeys[index]] as string
                const reading = sensors.read(sensorLabel)
                const rawValue = sensors.normalized(sensorLabel)
                smoothValues[index] += (rawValue - smoothValues[index]) * 0.08

                const baseColor = colorMode === 'Auto'
                    ? (reading?.unit === '°C' || reading?.unit === '°F'
                        ? colorByValue(smoothValues[index], sensorColors.temperature.gradient)
                        : reading?.unit === 'MB'
                          ? colorByValue(smoothValues[index], sensorColors.memory.gradient)
                          : colorByValue(smoothValues[index], sensorColors.load.gradient))
                    : accent
                const cardColor = lerpColor(baseColor, palette.fg.primary, 0.04)
                const cardSecondary = mixFaceAccent(cardColor, secondary, 0.32)
                const cardInk = resolveFaceInk(cardColor)

                card.style.setProperty('--accent', cardColor)
                card.style.setProperty('--secondary', cardSecondary)
                card.style.setProperty('--hero-ink', cardInk.hero)
                card.style.setProperty('--ui-ink', cardInk.ui)
                card.style.setProperty('--dim-ink', cardInk.dim)
                card.querySelector<HTMLElement>('.hc-sensor-grid__value')!.textContent = sensors.formatted(sensorLabel)
                card.querySelector<HTMLElement>('.hc-sensor-grid__label')!.textContent = humanizeSensorLabel(sensorLabel)
                card.querySelector<HTMLElement>('.hc-sensor-grid__chip')!.textContent = humanizeSensorLabel(sensorLabel)
                card.querySelector<HTMLElement>('.hc-sensor-grid__percent')!.textContent = `${Math.round(clamp01(smoothValues[index]) * 100)}%`
                const track = card.querySelector<HTMLElement>('.hc-sensor-grid__track')
                track!.style.display = controls.showTracks ? 'block' : 'none'
                card.querySelector<HTMLElement>('.hc-sensor-grid__track-fill')!.style.setProperty(
                    '--fill',
                    clamp01(smoothValues[index]).toFixed(4),
                )
            })

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)

            const points = [
                [W * 0.27, H * 0.28],
                [W * 0.73, H * 0.28],
                [W * 0.27, H * 0.72],
                [W * 0.73, H * 0.72],
            ] as const

            c.strokeStyle = withAlpha(ink.ui, 0.08)
            c.lineWidth = frameStyle === 'signal' ? 2.4 : 1.6
            points.forEach(([x, y]) => {
                c.beginPath()
                c.moveTo(W * 0.5, H * 0.5)
                c.lineTo(x, y)
                c.stroke()
            })

            const hub = c.createRadialGradient(W * 0.5, H * 0.5, 0, W * 0.5, H * 0.5, 34)
            hub.addColorStop(0, withAlpha(accent, 0.18))
            hub.addColorStop(1, withAlpha(accent, 0))
            c.fillStyle = hub
            c.beginPath()
            c.arc(W * 0.5, H * 0.5, 34, 0, Math.PI * 2)
            c.fill()

            points.forEach(([x, y], index) => {
                const cardColor = cards[index].style.getPropertyValue('--accent') || accent
                const glow = c.createRadialGradient(x, y, 0, x, y, frameStyle === 'radar' ? 78 : 56)
                glow.addColorStop(0, withAlpha(cardColor, 0.12))
                glow.addColorStop(1, withAlpha(cardColor, 0))
                c.fillStyle = glow
                c.fillRect(x - 80, y - 80, 160, 160)

                const sparkWidth = 132
                const sparkHeight = frameStyle === 'signal' ? 32 : 24
                sparkline(c, {
                    x: x - sparkWidth * 0.5,
                    y: y + 26,
                    width: sparkWidth,
                    height: sparkHeight,
                    values: histories[index].values(),
                    range: [0, 1],
                    color: cardColor,
                    lineWidth: frameStyle === 'signal' ? 2.1 : 1.6,
                    fillOpacity: 0.1,
                })
            })
        }
    },
)
