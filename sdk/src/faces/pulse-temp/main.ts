import {
    ValueHistory,
    arcGauge,
    color,
    colorByValue,
    combo,
    face,
    font,
    num,
    palette,
    sensor,
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

const STYLE_ID = 'hc-face-pulse-temp'
const FACE_SCHEMES = {
    temperature: ['#7ce9ff', '#ffb35f', '#ff6b7a'] as const,
    load: ['#50fa7b', '#00d4ff', '#ff5ca8'] as const,
    memory: ['#77ecff', '#8f70ff'] as const,
}

const STYLES = `
.hc-pulse-temp {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.coral};
    --hero-font: 'Orbitron', sans-serif;
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

.hc-pulse-temp__veil {
    display: none;
}

.hc-pulse-temp__stage {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 34px;
}

.hc-pulse-temp__stack {
    display: grid;
    gap: 16px;
    justify-items: center;
    text-align: center;
}

.hc-pulse-temp__eyebrow,
.hc-pulse-temp__meta {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    flex-wrap: wrap;
}

.hc-pulse-temp__chip {
    min-width: 0;
    padding: 8px 14px;
    border-radius: 999px;
    border: 1px solid var(--edge-ink);
    background: rgba(7, 8, 14, 0.28);
    font-family: var(--ui-font);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.22em;
    text-transform: uppercase;
    color: var(--ui-ink);
    backdrop-filter: blur(8px);
}

.hc-pulse-temp__chip--accent {
    border-color: color-mix(in srgb, var(--accent) 42%, transparent);
    color: var(--hero-ink);
    box-shadow: 0 0 20px color-mix(in srgb, var(--accent) 16%, transparent);
}

.hc-pulse-temp__hero {
    display: grid;
    gap: 12px;
    justify-items: center;
}

.hc-pulse-temp__value {
    display: flex;
    align-items: baseline;
    gap: 10px;
    font-family: var(--hero-font);
    font-size: 134px;
    font-weight: 700;
    line-height: 0.9;
    letter-spacing: 0.04em;
    color: var(--hero-ink);
    text-shadow:
        0 0 22px color-mix(in srgb, var(--accent) 16%, transparent),
        0 8px 30px rgba(0, 0, 0, 0.34);
}

.hc-pulse-temp__unit {
    font-family: var(--ui-font);
    font-size: 36px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-pulse-temp__meta-label {
    font-family: var(--ui-font);
    font-size: 10px;
    letter-spacing: 0.22em;
    text-transform: uppercase;
    color: var(--dim-ink);
}

.hc-pulse-temp__meta-value {
    font-family: var(--ui-font);
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--ui-ink);
}

.hc-pulse-temp__meta-group {
    display: grid;
    gap: 4px;
    justify-items: center;
}

.hc-pulse-temp[data-style='vector'] .hc-pulse-temp__value {
    letter-spacing: 0.08em;
}

.hc-pulse-temp[data-style='scope'] .hc-pulse-temp__chip--accent {
    background: color-mix(in srgb, var(--accent) 12%, rgba(7, 8, 14, 0.18));
}
`

export default face(
    'Pulse Temp',
    {
        targetSensor: sensor('Sensor', 'cpu_temp', { group: 'Data' }),
        colorScheme: combo('Color Scheme', ['Temperature', 'Load', 'Memory', 'Custom'], { group: 'Style' }),
        customColor: color('Custom Color', palette.neonCyan, { group: 'Style' }),
        meterStyle: combo('Meter Style', ['Halo', 'Vector', 'Scope'], { group: 'Layout' }),
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
                    meterStyle: 'Halo',
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
                    meterStyle: 'Vector',
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
                    meterStyle: 'Scope',
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
                    meterStyle: 'Halo',
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
                    meterStyle: 'Scope',
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
                    meterStyle: 'Vector',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                    glowIntensity: 44,
                },
            },
            {
                name: 'Amber Core',
                description: 'Warm gold thermal halo with clean sans meta.',
                controls: {
                    targetSensor: 'cpu_temp',
                    colorScheme: 'Custom',
                    customColor: '#ffb35c',
                    meterStyle: 'Halo',
                    heroFont: 'Exo 2',
                    uiFont: 'DM Sans',
                    glowIntensity: 62,
                },
            },
            {
                name: 'Prism Scope',
                description: 'Purple-cyan signal monitor with sharper tracking.',
                controls: {
                    targetSensor: 'cpu_load',
                    colorScheme: 'Custom',
                    customColor: '#7de5ff',
                    meterStyle: 'Scope',
                    heroFont: 'Orbitron',
                    uiFont: 'Space Grotesk',
                    glowIntensity: 78,
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
                <div class="hc-pulse-temp__stack">
                    <div class="hc-pulse-temp__eyebrow">
                        <div class="hc-pulse-temp__chip hc-pulse-temp__chip--accent hc-pulse-temp__mode">HALO</div>
                        <div class="hc-pulse-temp__chip hc-pulse-temp__trend">STEADY</div>
                    </div>
                    <div class="hc-pulse-temp__hero">
                        <div class="hc-pulse-temp__value"><span class="hc-pulse-temp__number">--</span><span class="hc-pulse-temp__unit">°C</span></div>
                        <div class="hc-pulse-temp__meta">
                            <div class="hc-pulse-temp__meta-group">
                                <div class="hc-pulse-temp__meta-label">Sensor</div>
                                <div class="hc-pulse-temp__meta-value hc-pulse-temp__sensor-name">CPU Temp</div>
                            </div>
                            <div class="hc-pulse-temp__meta-group">
                                <div class="hc-pulse-temp__meta-label">Peak</div>
                                <div class="hc-pulse-temp__meta-value hc-pulse-temp__peak">--</div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        `

        const numberEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__number')!
        const unitEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__unit')!
        const nameEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__sensor-name')!
        const modeEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__mode')!
        const trendEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__trend')!
        const peakEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__peak')!

        let smoothValue = 0
        let lastHistoryPush = 0
        let peakReading = Number.NEGATIVE_INFINITY
        let peakDisplay = '--'
        let activeSensor = ''
        let history = new ValueHistory(64)

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.5

        return (_time, controls, sensors) => {
            const sensorLabel = controls.targetSensor as string
            const reading = sensors.read(sensorLabel)
            const normalized = sensors.normalized(sensorLabel)
            smoothValue += (normalized - smoothValue) * 0.08

            if (activeSensor !== sensorLabel) {
                activeSensor = sensorLabel
                peakReading = Number.NEGATIVE_INFINITY
                peakDisplay = '--'
                history = new ValueHistory(64)
            }

            const scheme = controls.colorScheme as string
            const baseAccent = scheme === 'Temperature'
                ? colorByValue(smoothValue, FACE_SCHEMES.temperature)
                : scheme === 'Load'
                  ? colorByValue(smoothValue, FACE_SCHEMES.load)
                  : scheme === 'Memory'
                    ? colorByValue(smoothValue, FACE_SCHEMES.memory)
                    : (controls.customColor as string)
            const accent = baseAccent
            const secondary = scheme === 'Temperature'
                ? mixFaceAccent(baseAccent, palette.coral, 0.34)
                : scheme === 'Memory'
                  ? mixFaceAccent(baseAccent, palette.electricPurple, 0.48)
                  : mixFaceAccent(baseAccent)
            const ink = resolveFaceInk(accent)
            const panelColor = controls.panelColor as string
            const panelAlpha = controls.panelAlpha as number
            const backdrop = controls.backdrop as string
            const glow = clamp01((controls.glowIntensity as number) / 100)
            const meterStyle = (controls.meterStyle as string).toLowerCase()

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.panel = panelAlpha > 0 ? 'on' : 'off'
            root.dataset.style = meterStyle
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--hero-ink', ink.hero)
            root.style.setProperty('--ui-ink', ink.ui)
            root.style.setProperty('--dim-ink', ink.dim)
            root.style.setProperty('--edge-ink', ink.edge)
            root.style.setProperty('--panel', resolveFaceSurface(backdrop, panelColor, panelAlpha))

            const formatted = sensors.formatted(sensorLabel)
            const match = formatted.match(/^([\d.]+)\s*(.*)$/)
            numberEl.textContent = match?.[1] ?? formatted
            unitEl.textContent = match?.[2] || (reading?.unit ?? '')
            nameEl.textContent = humanizeSensorLabel(sensorLabel)
            nameEl.style.display = controls.showLabel ? 'block' : 'none'
            modeEl.textContent = meterStyle.toUpperCase()

            if (_time - lastHistoryPush > 0.12) {
                history.push(normalized)
                lastHistoryPush = _time
            }
            if (reading?.value != null && reading.value >= peakReading) {
                peakReading = reading.value
                peakDisplay = formatted
            }
            peakEl.textContent = peakDisplay
            const values = history.values()
            const trendDelta = values.length > 8 ? smoothValue - values[Math.max(0, values.length - 8)] : 0
            trendEl.textContent = trendDelta > 0.018 ? 'RISING' : trendDelta < -0.018 ? 'COOLING' : 'STEADY'

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)

            const startAngle = meterStyle === 'vector' ? Math.PI * 0.9 : Math.PI * 0.68
            const sweep = meterStyle === 'vector'
                ? Math.PI * 1.18
                : meterStyle === 'scope'
                  ? Math.PI * 1.52
                  : Math.PI * 1.7
            const radius = meterStyle === 'vector' ? 152 : 164
            const thickness = meterStyle === 'vector' ? 20 : 16

            arcGauge(c, {
                cx,
                cy,
                radius,
                thickness,
                value: smoothValue,
                fillColor: [accent, secondary],
                trackColor: withAlpha(ink.ui, 0.12),
                startAngle,
                sweep,
                glow: 0.35 + glow * 0.55,
            })

            c.save()
            c.lineWidth = meterStyle === 'scope' ? 4 : 3
            c.lineCap = 'round'
            const tickCount = meterStyle === 'vector' ? 18 : meterStyle === 'scope' ? 24 : 32
            for (let index = 0; index < tickCount; index++) {
                const progress = tickCount <= 1 ? 0 : index / (tickCount - 1)
                const angle = startAngle + sweep * progress
                const inner = radius - thickness * (meterStyle === 'scope' ? 1.55 : 1.2)
                const outer = inner + (meterStyle === 'vector' ? 10 : 14)
                const powered = progress <= smoothValue
                c.strokeStyle = powered
                    ? withAlpha(index % 2 === 0 ? accent : secondary, 0.54 + glow * 0.24)
                    : withAlpha(ink.ui, 0.12)
                c.beginPath()
                c.moveTo(cx + Math.cos(angle) * inner, cy + Math.sin(angle) * inner)
                c.lineTo(cx + Math.cos(angle) * outer, cy + Math.sin(angle) * outer)
                c.stroke()
            }
            c.restore()

            const orbitRadius = meterStyle === 'vector' ? 110 : 122
            for (let index = 0; index < 8; index++) {
                const angle = _time * 0.42 + index * (Math.PI * 0.25)
                const px = cx + Math.cos(angle) * orbitRadius
                const py = cy + Math.sin(angle) * orbitRadius
                c.fillStyle = withAlpha(index % 3 === 0 ? secondary : accent, 0.08 + glow * 0.08)
                c.beginPath()
                c.arc(px, py, index % 3 === 0 ? 3.4 : 2.4, 0, Math.PI * 2)
                c.fill()
            }

            if (values.length > 4) {
                sparkline(c, {
                    x: cx - 92,
                    y: cy + 82,
                    width: 184,
                    height: meterStyle === 'scope' ? 44 : 32,
                    values,
                    range: [0, 1],
                    color: accent,
                    lineWidth: meterStyle === 'scope' ? 2.4 : 1.8,
                    fill: true,
                    fillOpacity: meterStyle === 'scope' ? 0.16 : 0.1,
                })
            }

            const sweepAngle = startAngle + sweep * smoothValue
            const pulseX = cx + Math.cos(sweepAngle) * radius
            const pulseY = cy + Math.sin(sweepAngle) * radius
            const pulse = c.createRadialGradient(pulseX, pulseY, 0, pulseX, pulseY, 28)
            pulse.addColorStop(0, withAlpha(accent, 0.42))
            pulse.addColorStop(1, withAlpha(accent, 0))
            c.fillStyle = pulse
            c.beginPath()
            c.arc(pulseX, pulseY, 28, 0, Math.PI * 2)
            c.fill()
        }
    },
)
