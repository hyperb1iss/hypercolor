import {
    ValueHistory,
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
    withGlow,
} from '@hypercolor/sdk'

import {
    DISPLAY_FONT_FAMILIES,
    UI_FONT_FAMILIES,
    clamp01,
    createFaceRoot,
    ensureFaceStyles,
    humanizeSensorLabel,
} from '../shared/dom'

const STYLE_ID = 'hc-face-pulse-temp'

const STYLES = `
.hc-pulse-temp {
    --accent: ${palette.neonCyan};
    --hero-font: 'Orbitron', sans-serif;
    --ui-font: 'Sora', sans-serif;
    --panel: rgba(10, 10, 18, 0.78);
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: ${palette.fg.primary};
}

.hc-pulse-temp__veil {
    position: absolute;
    inset: 18px;
    border-radius: 34px;
    border: 1px solid rgba(255,255,255,0.08);
    background:
        radial-gradient(circle at 16% 18%, rgba(255,255,255,0.1), transparent 34%),
        linear-gradient(180deg, rgba(255,255,255,0.05), rgba(255,255,255,0.01)),
        var(--panel);
    box-shadow:
        inset 0 1px 0 rgba(255,255,255,0.06),
        0 24px 64px rgba(0,0,0,0.42);
}

.hc-pulse-temp[data-backdrop='clear'] .hc-pulse-temp__veil {
    background: linear-gradient(180deg, rgba(255,255,255,0.05), rgba(255,255,255,0.02));
    box-shadow: none;
}

.hc-pulse-temp__layout {
    position: absolute;
    inset: 0;
    display: grid;
    grid-template-rows: auto 1fr auto;
    padding: 28px;
}

.hc-pulse-temp__topline,
.hc-pulse-temp__footer {
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

.hc-pulse-temp__chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-radius: 999px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(10,10,18,0.42);
    backdrop-filter: blur(16px);
}

.hc-pulse-temp__dot {
    width: 7px;
    height: 7px;
    border-radius: 999px;
    background: var(--accent);
    box-shadow: 0 0 18px var(--accent);
    animation: hcPulseTempBeat 1.1s ease-in-out infinite;
}

.hc-pulse-temp__center {
    display: grid;
    place-items: center;
}

.hc-pulse-temp__hero {
    position: relative;
    display: grid;
    gap: 10px;
    justify-items: center;
    width: min(82%, 360px);
    padding: 26px 22px 24px;
    border-radius: 30px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(10,10,18,0.34);
    backdrop-filter: blur(18px);
}

.hc-pulse-temp__value {
    display: flex;
    align-items: baseline;
    gap: 10px;
    font-family: var(--hero-font);
    font-size: 112px;
    font-weight: 700;
    line-height: 0.9;
    letter-spacing: 0.04em;
    text-shadow: 0 0 34px rgba(0,0,0,0.34);
}

.hc-pulse-temp__unit {
    font-family: var(--ui-font);
    font-size: 28px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.72);
}

.hc-pulse-temp__label {
    font-family: var(--ui-font);
    font-size: 13px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.68);
}

.hc-pulse-temp__range {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: center;
}

.hc-pulse-temp__range-pill {
    padding: 7px 10px;
    border-radius: 999px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(255,255,255,0.04);
    font-family: var(--ui-font);
    font-size: 11px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.68);
}

.hc-pulse-temp__spark {
    display: grid;
    gap: 8px;
    width: 100%;
}

.hc-pulse-temp__spark-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-family: var(--ui-font);
    font-size: 11px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.7);
}

.hc-pulse-temp__spark-strip {
    position: relative;
    height: 8px;
    border-radius: 999px;
    overflow: hidden;
    background: rgba(255,255,255,0.06);
}

.hc-pulse-temp__spark-fill {
    position: absolute;
    inset: 0 auto 0 0;
    width: calc(var(--fill, 0) * 100%);
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), rgba(255,255,255,0.85));
}

@keyframes hcPulseTempBeat {
    0%, 100% { transform: scale(0.88); opacity: 0.72; }
    50% { transform: scale(1.12); opacity: 1; }
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
        backdrop: combo('Backdrop', ['Opaque', 'Glass', 'Clear'], { group: 'Style' }),
        chrome: combo('Chrome', ['Halo', 'Prism', 'Ribbon'], { group: 'Style' }),
        glowIntensity: num('Glow', [0, 100], 80, { group: 'Style' }),
        showSparkline: toggle('Sparkline', true, { group: 'Layout' }),
        showLabel: toggle('Label', true, { group: 'Layout' }),
    },
    {
        description: 'A dramatic single-sensor centerpiece with a luxe hero readout, animated halo fields, and presets tuned for thermal, load, and memory moments.',
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
                    backdrop: 'Glass',
                    chrome: 'Halo',
                    glowIntensity: 86,
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
                    backdrop: 'Opaque',
                    chrome: 'Prism',
                    glowIntensity: 72,
                },
            },
            {
                name: 'Load Bloom',
                description: 'Green-magenta pulse for load-driven movement.',
                controls: {
                    targetSensor: 'cpu_load',
                    colorScheme: 'Load',
                    heroFont: 'Audiowide',
                    uiFont: 'DM Sans',
                    backdrop: 'Glass',
                    chrome: 'Ribbon',
                    glowIntensity: 82,
                },
            },
            {
                name: 'Memory Core',
                description: 'Clean violet memory monitor with clear glass.',
                controls: {
                    targetSensor: 'ram_used',
                    colorScheme: 'Memory',
                    heroFont: 'Exo 2',
                    uiFont: 'Inter',
                    backdrop: 'Clear',
                    chrome: 'Halo',
                    glowIntensity: 66,
                },
            },
            {
                name: 'Coral Signal',
                description: 'Custom coral readout with soft chrome.',
                controls: {
                    targetSensor: 'cpu_temp',
                    colorScheme: 'Custom',
                    customColor: palette.coral,
                    heroFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                    backdrop: 'Glass',
                    chrome: 'Prism',
                    glowIntensity: 78,
                },
            },
            {
                name: 'Mono Luxe',
                description: 'Sharper monospaced numerals and restrained motion.',
                controls: {
                    targetSensor: 'gpu_load',
                    colorScheme: 'Load',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                    backdrop: 'Opaque',
                    chrome: 'Ribbon',
                    glowIntensity: 58,
                },
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-pulse-temp')
        root.innerHTML = `
            <div class="hc-pulse-temp__veil"></div>
            <div class="hc-pulse-temp__layout">
                <div class="hc-pulse-temp__topline">
                    <div class="hc-pulse-temp__chip"><span class="hc-pulse-temp__dot"></span><span class="hc-pulse-temp__label-text">PRIMARY SENSOR</span></div>
                    <div class="hc-pulse-temp__chip hc-pulse-temp__scheme">THERMAL</div>
                </div>
                <div class="hc-pulse-temp__center">
                    <div class="hc-pulse-temp__hero">
                        <div class="hc-pulse-temp__value"><span class="hc-pulse-temp__number">--</span><span class="hc-pulse-temp__unit">°C</span></div>
                        <div class="hc-pulse-temp__label hc-pulse-temp__sensor-name">CPU Temp</div>
                        <div class="hc-pulse-temp__range">
                            <span class="hc-pulse-temp__range-pill hc-pulse-temp__min">MIN --</span>
                            <span class="hc-pulse-temp__range-pill hc-pulse-temp__max">MAX --</span>
                            <span class="hc-pulse-temp__range-pill hc-pulse-temp__live">LIVE --</span>
                        </div>
                    </div>
                </div>
                <div class="hc-pulse-temp__footer">
                    <div class="hc-pulse-temp__spark">
                        <div class="hc-pulse-temp__spark-head"><span>TREND</span><span class="hc-pulse-temp__spark-value">--</span></div>
                        <div class="hc-pulse-temp__spark-strip"><div class="hc-pulse-temp__spark-fill"></div></div>
                    </div>
                </div>
            </div>
        `

        const numberEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__number')!
        const unitEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__unit')!
        const nameEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__sensor-name')!
        const schemeEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__scheme')!
        const minEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__min')!
        const maxEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__max')!
        const liveEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__live')!
        const sparkValueEl = root.querySelector<HTMLSpanElement>('.hc-pulse-temp__spark-value')!
        const sparkFillEl = root.querySelector<HTMLDivElement>('.hc-pulse-temp__spark-fill')!

        const history = new ValueHistory(96)
        let smoothValue = 0
        let lastHistoryPush = 0

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.48

        return (time, controls, sensors) => {
            const sensorLabel = controls.targetSensor as string
            const reading = sensors.read(sensorLabel)
            const normalized = sensors.normalized(sensorLabel)
            smoothValue += (normalized - smoothValue) * 0.08
            if (time - lastHistoryPush > 0.22) {
                history.push(normalized)
                lastHistoryPush = time
            }

            const scheme = controls.colorScheme as string
            const accent = scheme === 'Temperature'
                ? colorByValue(smoothValue, sensorColors.temperature.gradient)
                : scheme === 'Load'
                  ? colorByValue(smoothValue, sensorColors.load.gradient)
                  : scheme === 'Memory'
                    ? colorByValue(smoothValue, sensorColors.memory.gradient)
                    : (controls.customColor as string)
            const backdrop = controls.backdrop as string
            const chrome = (controls.chrome as string).toLowerCase()
            const glow = clamp01((controls.glowIntensity as number) / 100)

            root.dataset.backdrop = backdrop.toLowerCase()
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty(
                '--panel',
                backdrop === 'Opaque'
                    ? withAlpha(palette.bg.deep, 0.94)
                    : backdrop === 'Glass'
                      ? withAlpha(palette.bg.deep, 0.48)
                      : withAlpha('#05060a', 0.12),
            )

            const formatted = sensors.formatted(sensorLabel)
            const match = formatted.match(/^([\\d.]+)\\s*(.*)$/)
            numberEl.textContent = match?.[1] ?? formatted
            unitEl.textContent = match?.[2] || (reading?.unit ?? '')
            nameEl.textContent = controls.showLabel ? humanizeSensorLabel(sensorLabel) : 'SIGNAL CHANNEL'
            schemeEl.textContent = `${scheme.toUpperCase()} MODE`
            minEl.textContent = `MIN ${reading ? Math.round(reading.min) : '--'}`
            maxEl.textContent = `MAX ${reading ? Math.round(reading.max) : '--'}`
            liveEl.textContent = `LIVE ${formatted}`
            sparkValueEl.textContent = `${Math.round(smoothValue * 100)}%`
            sparkFillEl.style.setProperty('--fill', smoothValue.toFixed(4))
            sparkFillEl.parentElement!.parentElement!.style.display = controls.showSparkline ? 'grid' : 'none'

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)
            if (backdrop === 'Opaque') {
                c.fillStyle = withAlpha(palette.bg.deep, 0.96)
                c.fillRect(0, 0, W, H)
            } else if (backdrop === 'Glass') {
                c.fillStyle = withAlpha(palette.bg.deep, 0.18)
                c.fillRect(0, 0, W, H)
            }

            const ambient = c.createRadialGradient(cx, cy, 10, cx, cy, W * 0.48)
            ambient.addColorStop(0, withAlpha(accent, 0.16 + glow * 0.18))
            ambient.addColorStop(0.52, withAlpha(accent, 0.06 + glow * 0.08))
            ambient.addColorStop(1, 'rgba(0,0,0,0)')
            c.fillStyle = ambient
            c.fillRect(0, 0, W, H)

            const radius = Math.min(W, H) * 0.28
            const pulse = 1 + Math.sin(time * (1.8 + smoothValue * 2.6)) * (0.018 + glow * 0.014)

            c.save()
            c.translate(cx, cy)
            c.scale(pulse, pulse)
            c.translate(-cx, -cy)

            c.lineCap = 'round'
            c.lineWidth = 10
            c.strokeStyle = withAlpha('#ffffff', 0.06)
            c.beginPath()
            c.arc(cx, cy, radius, Math.PI * 0.15, Math.PI * 1.85)
            c.stroke()

            withGlow(c, accent, glow * 1.1, () => {
                c.strokeStyle = accent
                c.beginPath()
                c.arc(cx, cy, radius, Math.PI * 0.15, Math.PI * (0.15 + smoothValue * 1.7))
                c.stroke()
            })

            if (chrome === 'prism') {
                for (let i = 0; i < 7; i++) {
                    const angle = time * 0.5 + i * 0.9
                    const x = cx + Math.cos(angle) * radius * 0.72
                    const y = cy + Math.sin(angle) * radius * 0.72
                    const orb = c.createRadialGradient(x, y, 0, x, y, 34)
                    orb.addColorStop(0, withAlpha(accent, 0.18))
                    orb.addColorStop(1, withAlpha(accent, 0))
                    c.fillStyle = orb
                    c.fillRect(x - 34, y - 34, 68, 68)
                }
            } else if (chrome === 'ribbon') {
                c.strokeStyle = withAlpha(accent, 0.22)
                c.lineWidth = 2
                c.beginPath()
                for (let x = 24; x <= W - 24; x += 18) {
                    const wave = Math.sin(time * 1.2 + x * 0.024) * 10
                    if (x === 24) c.moveTo(x, cy + radius + 54 + wave)
                    else c.lineTo(x, cy + radius + 54 + wave)
                }
                c.stroke()
            }

            c.restore()

            if (controls.showSparkline && history.length > 2) {
                const values = history.values()
                const sparkWidth = W - 64
                const sparkHeight = 52
                const sparkLeft = 32
                const sparkTop = H - 102

                c.beginPath()
                values.forEach((value, index) => {
                    const x = sparkLeft + (index / Math.max(1, values.length - 1)) * sparkWidth
                    const y = sparkTop + sparkHeight - value * sparkHeight
                    if (index === 0) c.moveTo(x, y)
                    else c.lineTo(x, y)
                })
                c.lineWidth = 2.5
                c.strokeStyle = accent
                withGlow(c, accent, glow * 0.7, () => c.stroke())
            }
        }
    },
)
