import {
    color,
    colorByValue,
    combo,
    face,
    font,
    num,
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
    resolveFaceCanvasWash,
    resolveFaceSurface,
} from '../shared/dom'

const STYLE_ID = 'hc-face-sensor-grid'

const STYLES = `
.hc-sensor-grid {
    --accent: ${palette.neonCyan};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Sora', sans-serif;
    --panel: transparent;
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: ${palette.fg.primary};
}

.hc-sensor-grid__panel {
    position: absolute;
    inset: 18px;
    border-radius: 32px;
    border: 1px solid transparent;
    background: transparent;
    box-shadow: none;
}

.hc-sensor-grid[data-panel='on'] .hc-sensor-grid__panel {
    border-color: rgba(255,255,255,0.08);
    background:
        radial-gradient(circle at 18% 18%, rgba(255,255,255,0.08), transparent 30%),
        linear-gradient(160deg, rgba(255,255,255,0.05), rgba(255,255,255,0.01)),
        var(--panel);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.06), 0 24px 64px rgba(0,0,0,0.42);
}

.hc-sensor-grid[data-panel='on'][data-backdrop='clear'] .hc-sensor-grid__panel {
    background:
        linear-gradient(180deg, rgba(255,255,255,0.05), rgba(255,255,255,0.02)),
        var(--panel);
    box-shadow: none;
}

.hc-sensor-grid__layout {
    position: absolute;
    inset: 0;
    padding: 32px;
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
    gap: 10px;
    align-content: center;
    justify-items: start;
}

.hc-sensor-grid__value {
    font-family: var(--hero-font);
    font-size: 64px;
    font-weight: 700;
    line-height: 0.92;
    letter-spacing: 0.04em;
    text-shadow: 0 0 28px rgba(0,0,0,0.32);
}

.hc-sensor-grid__label {
    font-family: var(--ui-font);
    font-size: 13px;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.7);
}

.hc-sensor-grid__track {
    position: relative;
    width: 100%;
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
                    heroFont: 'Audiowide',
                    uiFont: 'DM Sans',
                },
            },
            {
                name: 'Mono Ops',
                description: 'Sharp monospaced cards for clean telemetry.',
                controls: {
                    colorMode: 'Auto',
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
                    heroFont: 'Orbitron',
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

        const { width: W, height: H } = ctx

        return (_time, controls, sensors) => {
            const colorMode = controls.colorMode as string
            const accent = controls.accent as string
            const panelColor = controls.panelColor as string
            const panelAlpha = controls.panelAlpha as number
            const backdrop = controls.backdrop as string

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.panel = panelAlpha > 0 ? 'on' : 'off'
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--panel', resolveFaceSurface(backdrop, panelColor, panelAlpha))

            cards.forEach((card, index) => {
                const sensorLabel = controls[sensorKeys[index]] as string
                const reading = sensors.read(sensorLabel)
                const rawValue = sensors.normalized(sensorLabel)
                smoothValues[index] += (rawValue - smoothValues[index]) * 0.08

                const cardColor = colorMode === 'Auto'
                    ? (reading?.unit === '°C' || reading?.unit === '°F'
                        ? colorByValue(smoothValues[index], sensorColors.temperature.gradient)
                        : reading?.unit === 'MB'
                          ? colorByValue(smoothValues[index], sensorColors.memory.gradient)
                          : colorByValue(smoothValues[index], sensorColors.load.gradient))
                    : accent

                card.style.setProperty('--accent', cardColor)
                card.querySelector<HTMLElement>('.hc-sensor-grid__value')!.textContent = sensors.formatted(sensorLabel)
                card.querySelector<HTMLElement>('.hc-sensor-grid__label')!.textContent = humanizeSensorLabel(sensorLabel)
                const track = card.querySelector<HTMLElement>('.hc-sensor-grid__track')
                track!.style.display = controls.showTracks ? 'block' : 'none'
                card.querySelector<HTMLElement>('.hc-sensor-grid__track-fill')!.style.setProperty(
                    '--fill',
                    clamp01(smoothValues[index]).toFixed(4),
                )
            })

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)

            const wash = resolveFaceCanvasWash(backdrop, panelColor, panelAlpha)
            if (wash) {
                c.fillStyle = wash
                c.fillRect(0, 0, W, H)
            }

            const points = [
                [W * 0.28, H * 0.29],
                [W * 0.72, H * 0.29],
                [W * 0.28, H * 0.71],
                [W * 0.72, H * 0.71],
            ] as const

            points.forEach(([x, y]) => {
                const glow = c.createRadialGradient(x, y, 0, x, y, 64)
                glow.addColorStop(0, withAlpha(accent, 0.14))
                glow.addColorStop(1, withAlpha(accent, 0))
                c.fillStyle = glow
                c.fillRect(x - 64, y - 64, 128, 128)
            })
        }
    },
)
