import {
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
    toggle,
} from '@hypercolor/sdk'

import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    humanizeSensorLabel,
    mixFaceAccent,
    resolveFaceInk,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-sensor-grid'

const STYLES = `
.hc-sensor-grid {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.electricPurple};
    --hero-font: 'Rajdhani', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --value-size: 50;
    --label-size: 11;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
    display: grid;
    place-items: center;
}

.hc-sensor-grid__frame {
    position: relative;
}

.hc-sensor-grid__cards {
    position: relative;
    width: 100%;
    height: 100%;
}

.hc-sensor-grid__card {
    position: absolute;
    width: calc(50% - 7px);
    height: calc(50% - 7px);
    display: grid;
    grid-template-rows: 1fr;
    place-items: center;
    justify-items: center;
    text-align: center;
    padding: 8px;
    background: transparent;
    border: none;
}

.hc-sensor-grid__card-inner {
    display: grid;
    gap: 8px;
    justify-items: center;
    align-items: center;
    text-align: center;
    width: 100%;
}

.hc-sensor-grid__card:nth-child(1) {
    top: 0;
    left: 0;
}

.hc-sensor-grid__card:nth-child(2) {
    top: 0;
    right: 0;
}

.hc-sensor-grid__card:nth-child(3) {
    bottom: 0;
    left: 0;
}

.hc-sensor-grid__card:nth-child(4) {
    bottom: 0;
    right: 0;
}

.hc-sensor-grid__label {
    font-family: var(--ui-font);
    font-size: calc(var(--label-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    text-align: center;
    color: var(--ui-ink);
}

.hc-sensor-grid__value {
    font-family: var(--hero-font);
    font-size: calc(var(--value-size) * 1px);
    font-weight: 600;
    line-height: 0.9;
    letter-spacing: 0.015em;
    text-align: center;
    color: var(--hero-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
    text-shadow:
        0 0 18px color-mix(in srgb, var(--accent) 12%, transparent),
        0 8px 24px rgba(0,0,0,0.24);
}

.hc-sensor-grid__percent {
    font-family: var(--ui-font);
    font-size: calc(var(--label-size) * 1px);
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--dim-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
}

.hc-sensor-grid__track {
    position: relative;
    width: 80%;
    height: 6px;
    border-radius: 999px;
    overflow: hidden;
    background: rgba(255,255,255,0.06);
    justify-self: center;
}

.hc-sensor-grid__track-fill {
    position: absolute;
    inset: 0 auto 0 0;
    width: calc(var(--fill, 0) * 100%);
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), var(--secondary));
}

.hc-sensor-grid__hidden {
    display: none !important;
}
`

export default face(
    'Sensor Grid',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        colorMode: combo('Colors', ['Auto', 'Accent'], { group: 'Style' }),
        heroFont: font('Hero Font', 'Rajdhani', { families: [...DISPLAY_FONT_FAMILIES], group: 'Typography' }),
        labelSize: num('Label Size', [9, 20], 11, { group: 'Typography' }),
        sensor1: sensor('Top Left', 'cpu_temp', { group: 'Sensors' }),
        sensor2: sensor('Top Right', 'gpu_temp', { group: 'Sensors' }),
        sensor3: sensor('Bottom Left', 'cpu_load', { group: 'Sensors' }),
        sensor4: sensor('Bottom Right', 'ram_used', { group: 'Sensors' }),
        showLabels: toggle('Show Labels', true, { group: 'Elements' }),
        showPercents: toggle('Show Percents', false, { group: 'Elements' }),
        showTracks: toggle('Show Tracks', true, { group: 'Elements' }),
        showValues: toggle('Show Values', true, { group: 'Elements' }),
        uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography' }),
        valueSize: num('Value Size', [28, 84], 50, { group: 'Typography' }),
    },
    {
        author: 'Hypercolor',
        description: 'A readable four-panel dashboard. Every element is independently toggleable.',
        designBasis: { height: 480, width: 480 },
        presets: [
            {
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Rajdhani',
                    sensor1: 'cpu_temp',
                    sensor2: 'gpu_temp',
                    sensor3: 'cpu_load',
                    sensor4: 'ram_used',
                    uiFont: 'Inter',
                },
                description: 'Balanced cyan dashboard for CPU, GPU, load, and memory.',
                name: 'System Vitals',
            },
            {
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Roboto Condensed',
                    sensor1: 'cpu_temp',
                    sensor2: 'gpu_temp',
                    sensor3: 'cpu_temp',
                    sensor4: 'gpu_temp',
                    uiFont: 'Inter',
                },
                description: 'All-temperature layout with compact condensed numerals.',
                name: 'Thermal Club',
            },
            {
                controls: {
                    accent: '#9ae7ff',
                    colorMode: 'Accent',
                    heroFont: 'Exo 2',
                    uiFont: 'Inter',
                },
                description: 'Cool blue accent with airy type.',
                name: 'Arctic Rail',
            },
            {
                controls: {
                    accent: palette.coral,
                    colorMode: 'Accent',
                    heroFont: 'Exo 2',
                    uiFont: 'DM Sans',
                },
                description: 'Coral matrix with softer, clearer hierarchy.',
                name: 'Signal Pink',
            },
            {
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                },
                description: 'Sharp monospaced telemetry.',
                name: 'Mono Ops',
            },
            {
                controls: {
                    accent: palette.electricYellow,
                    colorMode: 'Accent',
                    heroFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                },
                description: 'Warm gold accent with restrained chrome.',
                name: 'Gold Deck',
            },
            {
                controls: {
                    colorMode: 'Auto',
                    heroFont: 'Rajdhani',
                    showLabels: false,
                    showPercents: false,
                    showTracks: false,
                    uiFont: 'Inter',
                },
                description: 'Just the big values, no labels or chrome.',
                name: 'Naked Numbers',
            },
            {
                controls: {
                    accent: '#ffb45f',
                    colorMode: 'Accent',
                    heroFont: 'Roboto Condensed',
                    uiFont: 'Space Grotesk',
                },
                description: 'Warm amber survey deck with centered readings.',
                name: 'Amber Atlas',
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-sensor-grid')
        root.innerHTML = `
            <div class="hc-sensor-grid__frame">
                <div class="hc-sensor-grid__cards">
                    ${Array.from(
                        { length: 4 },
                        () => `
                        <div class="hc-sensor-grid__card">
                            <div class="hc-sensor-grid__card-inner">
                                <div class="hc-sensor-grid__label">UNASSIGNED</div>
                                <div class="hc-sensor-grid__value">--</div>
                                <div class="hc-sensor-grid__percent">0%</div>
                                <div class="hc-sensor-grid__track"><div class="hc-sensor-grid__track-fill"></div></div>
                            </div>
                        </div>
                    `,
                    ).join('')}
                </div>
            </div>
        `

        const frameEl = root.querySelector<HTMLDivElement>('.hc-sensor-grid__frame')!
        const cards = Array.from(root.querySelectorAll<HTMLDivElement>('.hc-sensor-grid__card'))
        const sensorKeys = ['sensor1', 'sensor2', 'sensor3', 'sensor4'] as const
        const smoothValues = [0, 0, 0, 0]
        const safeSize = Math.round(Math.min(ctx.width, ctx.height) * 0.78)
        frameEl.style.width = `${safeSize}px`
        frameEl.style.height = `${safeSize}px`

        return (_time, controls, sensors) => {
            const colorMode = controls.colorMode as string
            const accent = lerpColor(controls.accent as string, palette.fg.primary, 0.04)
            const secondary = mixFaceAccent(accent)
            const ink = resolveFaceInk(accent)

            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--hero-ink', ink.hero)
            root.style.setProperty('--ui-ink', ink.ui)
            root.style.setProperty('--dim-ink', ink.dim)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--value-size', `${controls.valueSize as number}`)
            root.style.setProperty('--label-size', `${controls.labelSize as number}`)

            const showLabels = controls.showLabels as boolean
            const showValues = controls.showValues as boolean
            const showPercents = controls.showPercents as boolean
            const showTracks = controls.showTracks as boolean

            cards.forEach((card, index) => {
                const sensorLabel = controls[sensorKeys[index]] as string
                const reading = sensors.read(sensorLabel)
                const rawValue = sensors.normalized(sensorLabel)
                smoothValues[index] += (rawValue - smoothValues[index]) * 0.08

                const baseColor =
                    colorMode === 'Auto'
                        ? reading?.unit === '°C' || reading?.unit === '°F'
                            ? colorByValue(smoothValues[index], sensorColors.temperature.gradient)
                            : reading?.unit === 'MB'
                              ? colorByValue(smoothValues[index], sensorColors.memory.gradient)
                              : colorByValue(smoothValues[index], sensorColors.load.gradient)
                        : accent
                const cardColor = lerpColor(baseColor, palette.fg.primary, 0.04)
                const cardSecondary = mixFaceAccent(cardColor, secondary, 0.32)
                const cardInk = resolveFaceInk(cardColor)

                card.style.setProperty('--accent', cardColor)
                card.style.setProperty('--secondary', cardSecondary)
                card.style.setProperty('--hero-ink', cardInk.hero)
                card.style.setProperty('--ui-ink', cardInk.ui)
                card.style.setProperty('--dim-ink', cardInk.dim)

                const labelEl = card.querySelector<HTMLElement>('.hc-sensor-grid__label')!
                const valueEl = card.querySelector<HTMLElement>('.hc-sensor-grid__value')!
                const percentEl = card.querySelector<HTMLElement>('.hc-sensor-grid__percent')!
                const trackEl = card.querySelector<HTMLElement>('.hc-sensor-grid__track')!

                valueEl.textContent = sensors.formatted(sensorLabel)
                labelEl.textContent = humanizeSensorLabel(sensorLabel)
                percentEl.textContent = `${Math.round(clamp01(smoothValues[index]) * 100)}%`
                card.querySelector<HTMLElement>('.hc-sensor-grid__track-fill')!.style.setProperty(
                    '--fill',
                    clamp01(smoothValues[index]).toFixed(4),
                )

                labelEl.classList.toggle('hc-sensor-grid__hidden', !showLabels)
                valueEl.classList.toggle('hc-sensor-grid__hidden', !showValues)
                percentEl.classList.toggle('hc-sensor-grid__hidden', !showPercents)
                trackEl.classList.toggle('hc-sensor-grid__hidden', !showTracks)
            })

            const c = ctx.ctx
            c.clearRect(0, 0, ctx.width, ctx.height)
        }
    },
)
