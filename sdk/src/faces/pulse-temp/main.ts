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

const STYLE_ID = 'hc-face-pulse-temp'

const STYLES = `
.hc-pulse-temp {
    --accent: ${palette.neonCyan};
    --hero-font: 'Orbitron', sans-serif;
    --ui-font: 'Sora', sans-serif;
    --panel: transparent;
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: ${palette.fg.primary};
}

.hc-pulse-temp__veil {
    position: absolute;
    inset: 18px;
    border-radius: 34px;
    border: 1px solid transparent;
    background: transparent;
    box-shadow: none;
}

.hc-pulse-temp[data-panel='on'] .hc-pulse-temp__veil {
    border-color: rgba(255,255,255,0.08);
    background:
        radial-gradient(circle at 16% 18%, rgba(255,255,255,0.1), transparent 34%),
        linear-gradient(180deg, rgba(255,255,255,0.05), rgba(255,255,255,0.01)),
        var(--panel);
    box-shadow:
        inset 0 1px 0 rgba(255,255,255,0.06),
        0 24px 64px rgba(0,0,0,0.42);
}

.hc-pulse-temp[data-panel='on'][data-backdrop='clear'] .hc-pulse-temp__veil {
    background:
        linear-gradient(180deg, rgba(255,255,255,0.05), rgba(255,255,255,0.02)),
        var(--panel);
    box-shadow: none;
}

.hc-pulse-temp__stage {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 28px;
}

.hc-pulse-temp__hero {
    display: grid;
    gap: 10px;
    justify-items: center;
    text-align: center;
}

.hc-pulse-temp__value {
    display: flex;
    align-items: baseline;
    gap: 10px;
    font-family: var(--hero-font);
    font-size: 144px;
    font-weight: 700;
    line-height: 0.9;
    letter-spacing: 0.04em;
    text-shadow: 0 0 40px rgba(0, 0, 0, 0.4);
}

.hc-pulse-temp__unit {
    font-family: var(--ui-font);
    font-size: 36px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: rgba(232, 230, 240, 0.72);
}

.hc-pulse-temp__label {
    font-family: var(--ui-font);
    font-size: 14px;
    letter-spacing: 0.24em;
    text-transform: uppercase;
    color: rgba(232, 230, 240, 0.6);
}
`

export default face(
    'Pulse Temp',
    {
        targetSensor: sensor('Sensor', 'cpu_temp', { group: 'Data' }),
        colorScheme: combo('Color Scheme', ['Temperature', 'Load', 'Memory', 'Custom'], { group: 'Style' }),
        customColor: color('Custom Color', palette.neonCyan, { group: 'Style' }),
        heroFont: font('Hero Font', 'Orbitron', { group: 'Typography', families: [...DISPLAY_FONT_FAMILIES] }),
        uiFont: font('UI Font', 'Sora', { group: 'Typography', families: [...UI_FONT_FAMILIES] }),
        panelColor: color('Panel Color', palette.bg.deep, { group: 'Style' }),
        panelAlpha: num('Panel Alpha', [0, 100], 0, { group: 'Style' }),
        backdrop: combo('Backdrop', ['Clear', 'Glass', 'Opaque'], { group: 'Style' }),
        glowIntensity: num('Glow', [0, 100], 60, { group: 'Style' }),
        showLabel: toggle('Label', true, { group: 'Layout' }),
    },
    {
        description: 'A dramatic single-sensor centerpiece with a luxe hero readout and color tuned to thermal, load, and memory moments.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'CPU Siren',
                description: 'Cyan-to-hot thermal watch with Orbitron chrome.',
                controls: {
                    targetSensor: 'cpu_temp',
                    colorScheme: 'Temperature',
                    heroFont: 'Orbitron',
                    uiFont: 'Sora',
                    glowIntensity: 70,
                },
            },
            {
                name: 'GPU Ember',
                description: 'Warm overclock mood with bold condensed numerals.',
                controls: {
                    targetSensor: 'gpu_temp',
                    colorScheme: 'Temperature',
                    heroFont: 'Bebas Neue',
                    uiFont: 'Roboto Condensed',
                    glowIntensity: 64,
                },
            },
            {
                name: 'Load Bloom',
                description: 'Green-magenta gradient for load-driven movement.',
                controls: {
                    targetSensor: 'cpu_load',
                    colorScheme: 'Load',
                    heroFont: 'Audiowide',
                    uiFont: 'DM Sans',
                    glowIntensity: 72,
                },
            },
            {
                name: 'Memory Core',
                description: 'Clean violet memory monitor.',
                controls: {
                    targetSensor: 'ram_used',
                    colorScheme: 'Memory',
                    heroFont: 'Exo 2',
                    uiFont: 'Inter',
                    glowIntensity: 54,
                },
            },
            {
                name: 'Coral Signal',
                description: 'Custom coral readout.',
                controls: {
                    targetSensor: 'cpu_temp',
                    colorScheme: 'Custom',
                    customColor: palette.coral,
                    heroFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                    glowIntensity: 68,
                },
            },
            {
                name: 'Mono Luxe',
                description: 'Sharper monospaced numerals.',
                controls: {
                    targetSensor: 'gpu_load',
                    colorScheme: 'Load',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                    glowIntensity: 44,
                },
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-pulse-temp')
        root.innerHTML = `
            <div class="hc-pulse-temp__veil"></div>
            <div class="hc-pulse-temp__stage">
                <div class="hc-pulse-temp__hero">
                    <div class="hc-pulse-temp__value"><span class="hc-pulse-temp__number">--</span><span class="hc-pulse-temp__unit">°C</span></div>
                    <div class="hc-pulse-temp__label hc-pulse-temp__sensor-name">CPU Temp</div>
                </div>
            </div>
        `

        const numberEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__number')!
        const unitEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__unit')!
        const nameEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__sensor-name')!

        let smoothValue = 0

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (_time, controls, sensors) => {
            const sensorLabel = controls.targetSensor as string
            const reading = sensors.read(sensorLabel)
            const normalized = sensors.normalized(sensorLabel)
            smoothValue += (normalized - smoothValue) * 0.08

            const scheme = controls.colorScheme as string
            const accent = scheme === 'Temperature'
                ? colorByValue(smoothValue, sensorColors.temperature.gradient)
                : scheme === 'Load'
                  ? colorByValue(smoothValue, sensorColors.load.gradient)
                  : scheme === 'Memory'
                    ? colorByValue(smoothValue, sensorColors.memory.gradient)
                    : (controls.customColor as string)
            const panelColor = controls.panelColor as string
            const panelAlpha = controls.panelAlpha as number
            const backdrop = controls.backdrop as string
            const glow = clamp01((controls.glowIntensity as number) / 100)

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.panel = panelAlpha > 0 ? 'on' : 'off'
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--panel', resolveFaceSurface(backdrop, panelColor, panelAlpha))

            const formatted = sensors.formatted(sensorLabel)
            const match = formatted.match(/^([\d.]+)\s*(.*)$/)
            numberEl.textContent = match?.[1] ?? formatted
            unitEl.textContent = match?.[2] || (reading?.unit ?? '')
            nameEl.textContent = humanizeSensorLabel(sensorLabel)
            nameEl.style.display = controls.showLabel ? 'block' : 'none'

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)

            const wash = resolveFaceCanvasWash(backdrop, panelColor, panelAlpha)
            if (wash) {
                c.fillStyle = wash
                c.fillRect(0, 0, W, H)
            }

            const ambient = c.createRadialGradient(cx, cy, 20, cx, cy, W * 0.5)
            ambient.addColorStop(0, withAlpha(accent, 0.16 + glow * 0.2))
            ambient.addColorStop(0.55, withAlpha(accent, 0.05 + glow * 0.06))
            ambient.addColorStop(1, 'rgba(0,0,0,0)')
            c.fillStyle = ambient
            c.fillRect(0, 0, W, H)
        }
    },
)
