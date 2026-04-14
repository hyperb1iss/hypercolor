import {
    color,
    colorByValue,
    combo,
    face,
    font,
    palette,
    sensor,
    sensorColors,
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
} from '../shared/dom'

const STYLE_ID = 'hc-face-sensor-grid'

const STYLES = `
.hc-sensor-grid {
    --accent: ${palette.neonCyan};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Sora', sans-serif;
    --panel: rgba(10, 10, 18, 0.84);
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: ${palette.fg.primary};
}

.hc-sensor-grid__panel {
    position: absolute;
    inset: 18px;
    border-radius: 32px;
    border: 1px solid rgba(255,255,255,0.08);
    background:
        radial-gradient(circle at 18% 18%, rgba(255,255,255,0.08), transparent 30%),
        linear-gradient(160deg, rgba(255,255,255,0.05), rgba(255,255,255,0.01)),
        var(--panel);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.06), 0 24px 64px rgba(0,0,0,0.42);
}

.hc-sensor-grid[data-backdrop='clear'] .hc-sensor-grid__panel {
    background: linear-gradient(180deg, rgba(255,255,255,0.05), rgba(255,255,255,0.02));
    box-shadow: none;
}

.hc-sensor-grid__layout {
    position: absolute;
    inset: 0;
    display: grid;
    grid-template-rows: auto 1fr;
    gap: 18px;
    padding: 26px;
}

.hc-sensor-grid__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    font-family: var(--ui-font);
    font-size: 11px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.72);
}

.hc-sensor-grid__header-pill {
    padding: 8px 12px;
    border-radius: 999px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(10,10,18,0.42);
    backdrop-filter: blur(16px);
}

.hc-sensor-grid__cards {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    grid-template-rows: repeat(2, minmax(0, 1fr));
    gap: 14px;
}

.hc-sensor-grid[data-layout='ribbon'] .hc-sensor-grid__cards {
    gap: 10px;
}

.hc-sensor-grid__card {
    position: relative;
    display: grid;
    gap: 12px;
    align-content: start;
    padding: 16px;
    border-radius: 24px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(255,255,255,0.04);
    backdrop-filter: blur(14px);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.04);
}

.hc-sensor-grid__card::after {
    content: '';
    position: absolute;
    inset: auto 16px 16px;
    height: 3px;
    border-radius: 999px;
    background: rgba(255,255,255,0.06);
}

.hc-sensor-grid__card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    font-family: var(--ui-font);
    font-size: 10px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.64);
}

.hc-sensor-grid__value {
    font-family: var(--hero-font);
    font-size: 46px;
    font-weight: 700;
    line-height: 0.92;
    letter-spacing: 0.04em;
    text-shadow: 0 0 24px rgba(0,0,0,0.3);
}

.hc-sensor-grid__label {
    font-family: var(--ui-font);
    font-size: 12px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.72);
}

.hc-sensor-grid__track {
    position: relative;
    height: 8px;
    border-radius: 999px;
    overflow: hidden;
    background: rgba(255,255,255,0.06);
}

.hc-sensor-grid__track-fill {
    position: absolute;
    inset: 0 auto 0 0;
    width: calc(var(--fill, 0) * 100%);
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), rgba(255,255,255,0.88));
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
        heroFont: font('Hero Font', 'Rajdhani', { group: 'Typography', families: [...DISPLAY_FONT_FAMILIES] }),
        uiFont: font('UI Font', 'Sora', { group: 'Typography', families: [...UI_FONT_FAMILIES] }),
        layoutStyle: combo('Layout', ['Matrix', 'Ribbon', 'Radar'], { group: 'Style' }),
        backdrop: combo('Backdrop', ['Opaque', 'Glass', 'Clear'], { group: 'Style' }),
        showTracks: toggle('Tracks', true, { group: 'Style' }),
    },
    {
        description: 'A modular four-panel dashboard with richer card styling, flexible typography, and presets for vitals, thermals, load, and memory snapshots.',
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
                    heroFont: 'Rajdhani',
                    uiFont: 'Sora',
                    layoutStyle: 'Matrix',
                    backdrop: 'Glass',
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
                    heroFont: 'Bebas Neue',
                    uiFont: 'Roboto Condensed',
                    layoutStyle: 'Ribbon',
                    backdrop: 'Opaque',
                },
            },
            {
                name: 'Radar Ice',
                description: 'Cool blue accent with airy chrome.',
                controls: {
                    colorMode: 'Accent',
                    accent: '#9ae7ff',
                    heroFont: 'Exo 2',
                    uiFont: 'Inter',
                    layoutStyle: 'Radar',
                    backdrop: 'Clear',
                },
            },
            {
                name: 'Signal Pink',
                description: 'Femme coral matrix with bold display font.',
                controls: {
                    colorMode: 'Accent',
                    accent: palette.coral,
                    heroFont: 'Audiowide',
                    uiFont: 'DM Sans',
                    layoutStyle: 'Matrix',
                    backdrop: 'Glass',
                },
            },
            {
                name: 'Mono Ops',
                description: 'Sharp monospaced cards for clean telemetry.',
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                    layoutStyle: 'Ribbon',
                    backdrop: 'Opaque',
                },
            },
            {
                name: 'Gold Deck',
                description: 'Warm gold accent with polished chrome.',
                controls: {
                    colorMode: 'Accent',
                    accent: palette.electricYellow,
                    heroFont: 'Orbitron',
                    uiFont: 'Space Grotesk',
                    layoutStyle: 'Radar',
                    backdrop: 'Glass',
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
                <div class="hc-sensor-grid__header">
                    <div class="hc-sensor-grid__header-pill">DISPLAY DASHBOARD</div>
                    <div class="hc-sensor-grid__header-pill hc-sensor-grid__mode">AUTO COLORS</div>
                </div>
                <div class="hc-sensor-grid__cards">
                    ${Array.from({ length: 4 }, (_, index) => `
                        <div class="hc-sensor-grid__card" data-card="${index}">
                            <div class="hc-sensor-grid__card-head">
                                <span class="hc-sensor-grid__index">0${index + 1}</span>
                                <span class="hc-sensor-grid__unit">--</span>
                            </div>
                            <div class="hc-sensor-grid__value">--</div>
                            <div class="hc-sensor-grid__label">UNASSIGNED</div>
                            <div class="hc-sensor-grid__track"><div class="hc-sensor-grid__track-fill"></div></div>
                        </div>
                    `).join('')}
                </div>
            </div>
        `

        const modeEl = root.querySelector<HTMLDivElement>('.hc-sensor-grid__mode')!
        const cards = Array.from(root.querySelectorAll<HTMLDivElement>('.hc-sensor-grid__card'))
        const sensorKeys = ['sensor1', 'sensor2', 'sensor3', 'sensor4'] as const
        const smoothValues = [0, 0, 0, 0]

        const { width: W, height: H } = ctx

        return (time, controls, sensors) => {
            const colorMode = controls.colorMode as string
            const accent = controls.accent as string
            const layoutStyle = (controls.layoutStyle as string).toLowerCase()
            const backdrop = controls.backdrop as string

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.layout = layoutStyle
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty(
                '--panel',
                backdrop === 'Opaque'
                    ? withAlpha(palette.bg.deep, 0.94)
                    : backdrop === 'Glass'
                      ? withAlpha(palette.bg.deep, 0.5)
                      : withAlpha('#05060a', 0.12),
            )
            modeEl.textContent = colorMode === 'Auto' ? 'AUTO COLORS' : 'ACCENT LOCK'

            cards.forEach((card, index) => {
                const sensorLabel = controls[sensorKeys[index]] as string
                const reading = sensors.read(sensorLabel)
                const rawValue = sensors.normalized(sensorLabel)
                smoothValues[index] += (rawValue - smoothValues[index]) * 0.08

                const color = colorMode === 'Auto'
                    ? (reading?.unit === '°C' || reading?.unit === '°F'
                        ? colorByValue(smoothValues[index], sensorColors.temperature.gradient)
                        : reading?.unit === 'MB'
                          ? colorByValue(smoothValues[index], sensorColors.memory.gradient)
                          : colorByValue(smoothValues[index], sensorColors.load.gradient))
                    : accent

                card.style.setProperty('--accent', color)
                card.querySelector<HTMLElement>('.hc-sensor-grid__value')!.textContent = sensors.formatted(sensorLabel)
                card.querySelector<HTMLElement>('.hc-sensor-grid__label')!.textContent = humanizeSensorLabel(sensorLabel)
                card.querySelector<HTMLElement>('.hc-sensor-grid__unit')!.textContent = reading?.unit ?? '--'
                const track = card.querySelector<HTMLElement>('.hc-sensor-grid__track')
                track!.style.display = controls.showTracks ? 'block' : 'none'
                card.querySelector<HTMLElement>('.hc-sensor-grid__track-fill')!.style.setProperty(
                    '--fill',
                    clamp01(smoothValues[index]).toFixed(4),
                )
            })

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)
            if (backdrop === 'Opaque') {
                c.fillStyle = withAlpha(palette.bg.deep, 0.96)
                c.fillRect(0, 0, W, H)
            } else if (backdrop === 'Glass') {
                c.fillStyle = withAlpha(palette.bg.deep, 0.18)
                c.fillRect(0, 0, W, H)
            }

            const points = [
                [W * 0.28, H * 0.29],
                [W * 0.72, H * 0.29],
                [W * 0.28, H * 0.71],
                [W * 0.72, H * 0.71],
            ] as const

            c.lineWidth = layoutStyle === 'ribbon' ? 2 : 1
            c.strokeStyle = withAlpha(accent, layoutStyle === 'radar' ? 0.18 : 0.12)
            c.beginPath()
            c.moveTo(points[0][0], points[0][1])
            c.lineTo(points[1][0], points[1][1])
            c.lineTo(points[3][0], points[3][1])
            c.lineTo(points[2][0], points[2][1])
            c.closePath()
            c.stroke()

            points.forEach(([x, y], index) => {
                const glow = c.createRadialGradient(x, y, 0, x, y, 52)
                glow.addColorStop(0, withAlpha(accent, 0.18))
                glow.addColorStop(1, withAlpha(accent, 0))
                c.fillStyle = glow
                c.fillRect(x - 52, y - 52, 104, 104)

                if (layoutStyle === 'radar') {
                    const angle = time * 0.65 + index * 1.2
                    c.strokeStyle = withAlpha(accent, 0.14)
                    c.beginPath()
                    c.moveTo(x, y)
                    c.lineTo(x + Math.cos(angle) * 36, y + Math.sin(angle) * 36)
                    c.stroke()
                }
            })

            if (layoutStyle === 'ribbon') {
                c.strokeStyle = withAlpha(accent, 0.18)
                c.beginPath()
                for (let x = 20; x <= W - 20; x += 16) {
                    const y = H * 0.5 + Math.sin(time * 1.1 + x * 0.03) * 14
                    if (x === 20) c.moveTo(x, y)
                    else c.lineTo(x, y)
                }
                c.stroke()
            }
        }
    },
)
