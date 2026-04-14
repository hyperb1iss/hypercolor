import {
    ValueHistory,
    color,
    combo,
    face,
    font,
    num,
    palette,
    sensor,
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

const STYLE_ID = 'hc-face-silkcircuit-hud'

const STYLES = `
.hc-silk-hud {
    --accent: ${palette.neonCyan};
    --secondary: ${palette.coral};
    --hero-font: 'Orbitron', sans-serif;
    --ui-font: 'Sora', sans-serif;
    --panel: transparent;
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: ${palette.fg.primary};
}

.hc-silk-hud__panel {
    position: absolute;
    inset: 18px;
    border-radius: 34px;
    border: 1px solid transparent;
    background: transparent;
    box-shadow: none;
}

.hc-silk-hud[data-panel='on'] .hc-silk-hud__panel {
    border-color: rgba(255,255,255,0.08);
    background:
        radial-gradient(circle at 16% 18%, rgba(255,255,255,0.08), transparent 30%),
        linear-gradient(180deg, rgba(255,255,255,0.04), rgba(255,255,255,0.01)),
        var(--panel);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.06), 0 24px 64px rgba(0,0,0,0.42);
}

.hc-silk-hud[data-panel='on'][data-backdrop='clear'] .hc-silk-hud__panel {
    background:
        linear-gradient(180deg, rgba(255,255,255,0.05), rgba(255,255,255,0.02)),
        var(--panel);
    box-shadow: none;
}

.hc-silk-hud__layout {
    position: absolute;
    inset: 0;
    display: grid;
    grid-template-rows: auto auto 1fr;
    gap: 20px;
    padding: 32px;
}

.hc-silk-hud__clock {
    display: grid;
    gap: 4px;
}

.hc-silk-hud__time {
    font-family: var(--hero-font);
    font-size: 60px;
    line-height: 0.94;
    letter-spacing: 0.08em;
    text-transform: uppercase;
}

.hc-silk-hud__date {
    font-family: var(--ui-font);
    font-size: 12px;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.6);
}

.hc-silk-hud__hero {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 28px;
}

.hc-silk-hud__metric {
    display: grid;
    gap: 4px;
}

.hc-silk-hud__metric-label {
    font-family: var(--ui-font);
    font-size: 11px;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.6);
}

.hc-silk-hud__metric-value {
    font-family: var(--hero-font);
    font-size: 64px;
    line-height: 0.94;
}

.hc-silk-hud__bars {
    display: grid;
    gap: 16px;
    align-content: end;
}

.hc-silk-hud__bar {
    display: grid;
    gap: 6px;
}

.hc-silk-hud__bar-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-family: var(--ui-font);
    font-size: 11px;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    color: rgba(232,230,240,0.7);
}

.hc-silk-hud__bar-rail {
    position: relative;
    height: 10px;
    border-radius: 999px;
    overflow: hidden;
    background: rgba(255,255,255,0.06);
}

.hc-silk-hud__bar-fill {
    position: absolute;
    inset: 0 auto 0 0;
    width: calc(var(--fill, 0) * 100%);
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), var(--secondary));
}
`

export default face(
    'SilkCircuit HUD',
    {
        cpuTempSensor: sensor('CPU Temp Sensor', 'cpu_temp', { group: 'Sensors' }),
        gpuTempSensor: sensor('GPU Temp Sensor', 'gpu_temp', { group: 'Sensors' }),
        cpuLoadSensor: sensor('CPU Load Sensor', 'cpu_load', { group: 'Sensors' }),
        ramSensor: sensor('RAM Sensor', 'ram_used', { group: 'Sensors' }),
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        secondaryAccent: color('Secondary', palette.coral, { group: 'Style' }),
        heroFont: font('Hero Font', 'Orbitron', { group: 'Typography', families: [...DISPLAY_FONT_FAMILIES] }),
        uiFont: font('UI Font', 'Sora', { group: 'Typography', families: [...UI_FONT_FAMILIES] }),
        panelColor: color('Panel Color', palette.bg.deep, { group: 'Style' }),
        panelAlpha: num('Panel Alpha', [0, 100], 0, { group: 'Style' }),
        backdrop: combo('Backdrop', ['Clear', 'Glass', 'Opaque'], { group: 'Style' }),
        hourFormat: combo('Clock Format', ['24h', '12h'], { group: 'Clock' }),
        showDate: toggle('Show Date', true, { group: 'Clock' }),
        showBars: toggle('Show Bars', true, { group: 'Layout' }),
    },
    {
        description: 'A flagship command-center face with strong typography, layered hero metrics, and presets with distinct moods.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'Signature HUD',
                description: 'The classic SilkCircuit cyan/coral command deck.',
                controls: {
                    accent: palette.neonCyan,
                    secondaryAccent: palette.coral,
                    heroFont: 'Orbitron',
                    uiFont: 'Sora',
                },
            },
            {
                name: 'Forge Deck',
                description: 'Warm amber chrome and bold numerals.',
                controls: {
                    accent: '#ffb347',
                    secondaryAccent: '#ff6b6b',
                    heroFont: 'Bebas Neue',
                    uiFont: 'Roboto Condensed',
                },
            },
            {
                name: 'Arctic Rail',
                description: 'Cool blue minimal HUD with airy type.',
                controls: {
                    accent: '#9ae7ff',
                    secondaryAccent: '#c8d5ff',
                    heroFont: 'Exo 2',
                    uiFont: 'Inter',
                },
            },
            {
                name: 'Rose Protocol',
                description: 'Coral-forward femme variant.',
                controls: {
                    accent: palette.coral,
                    secondaryAccent: '#ffb8dd',
                    heroFont: 'Audiowide',
                    uiFont: 'DM Sans',
                },
            },
            {
                name: 'Mono Grid',
                description: 'Sharper monospaced telemetry.',
                controls: {
                    accent: palette.electricYellow,
                    secondaryAccent: '#ffa166',
                    heroFont: 'Space Mono',
                    uiFont: 'JetBrains Mono',
                },
            },
            {
                name: 'Nightclub Ops',
                description: 'Dark magenta-blue control room.',
                controls: {
                    accent: '#ff4da6',
                    secondaryAccent: '#6a8bff',
                    heroFont: 'Rajdhani',
                    uiFont: 'Space Grotesk',
                },
            },
        ],
    },
    (ctx) => {
        ensureFaceStyles(STYLE_ID, STYLES)
        const root = createFaceRoot(ctx, 'hc-silk-hud')
        root.innerHTML = `
            <div class="hc-silk-hud__panel"></div>
            <div class="hc-silk-hud__layout">
                <div class="hc-silk-hud__clock">
                    <div class="hc-silk-hud__time">00:00</div>
                    <div class="hc-silk-hud__date">MON MAY 15</div>
                </div>
                <div class="hc-silk-hud__hero">
                    <div class="hc-silk-hud__metric hc-silk-hud__cpu">
                        <div class="hc-silk-hud__metric-label">CPU TEMP</div>
                        <div class="hc-silk-hud__metric-value">--</div>
                    </div>
                    <div class="hc-silk-hud__metric hc-silk-hud__gpu">
                        <div class="hc-silk-hud__metric-label">GPU TEMP</div>
                        <div class="hc-silk-hud__metric-value">--</div>
                    </div>
                </div>
                <div class="hc-silk-hud__bars">
                    <div class="hc-silk-hud__bar">
                        <div class="hc-silk-hud__bar-head"><span class="hc-silk-hud__load-label">CPU LOAD</span><span class="hc-silk-hud__load-value">--</span></div>
                        <div class="hc-silk-hud__bar-rail"><div class="hc-silk-hud__bar-fill hc-silk-hud__load-fill"></div></div>
                    </div>
                    <div class="hc-silk-hud__bar">
                        <div class="hc-silk-hud__bar-head"><span class="hc-silk-hud__ram-label">RAM</span><span class="hc-silk-hud__ram-value">--</span></div>
                        <div class="hc-silk-hud__bar-rail"><div class="hc-silk-hud__bar-fill hc-silk-hud__ram-fill"></div></div>
                    </div>
                </div>
            </div>
        `

        const timeEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__time')!
        const dateEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__date')!
        const cpuValueEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__cpu .hc-silk-hud__metric-value')!
        const gpuValueEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__gpu .hc-silk-hud__metric-value')!
        const loadValueEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__load-value')!
        const ramValueEl = root.querySelector<HTMLSpanElement>('.hc-silk-hud__ram-value')!
        const loadFillEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__load-fill')!
        const ramFillEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__ram-fill')!
        const barsEl = root.querySelector<HTMLDivElement>('.hc-silk-hud__bars')!

        const cpuHistory = new ValueHistory(72)
        const gpuHistory = new ValueHistory(72)
        let smoothCpu = 0
        let smoothGpu = 0
        let smoothLoad = 0
        let smoothRam = 0
        let lastHistoryPush = 0

        const { width: W, height: H } = ctx
        const cx = W * 0.5
        const cy = H * 0.58

        return (time, controls, sensors) => {
            const accent = controls.accent as string
            const secondary = controls.secondaryAccent as string
            const panelColor = controls.panelColor as string
            const panelAlpha = controls.panelAlpha as number
            const backdrop = controls.backdrop as string

            root.dataset.backdrop = backdrop.toLowerCase()
            root.dataset.panel = panelAlpha > 0 ? 'on' : 'off'
            root.style.setProperty('--accent', accent)
            root.style.setProperty('--secondary', secondary)
            root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
            root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)
            root.style.setProperty('--panel', resolveFaceSurface(backdrop, panelColor, panelAlpha))

            const cpuTemp = sensors.normalized(controls.cpuTempSensor as string)
            const gpuTemp = sensors.normalized(controls.gpuTempSensor as string)
            const cpuLoad = sensors.normalized(controls.cpuLoadSensor as string)
            const ram = sensors.normalized(controls.ramSensor as string)
            smoothCpu += (cpuTemp - smoothCpu) * 0.08
            smoothGpu += (gpuTemp - smoothGpu) * 0.08
            smoothLoad += (cpuLoad - smoothLoad) * 0.12
            smoothRam += (ram - smoothRam) * 0.1

            if (time - lastHistoryPush > 0.25) {
                cpuHistory.push(cpuTemp)
                gpuHistory.push(gpuTemp)
                lastHistoryPush = time
            }

            const now = new Date()
            let hours = now.getHours()
            const minutes = now.getMinutes()
            if (controls.hourFormat === '12h') hours = hours % 12 || 12
            timeEl.textContent = `${hours.toString().padStart(2, '0')}:${minutes
                .toString()
                .padStart(2, '0')}`
            dateEl.textContent = controls.showDate
                ? now
                      .toLocaleDateString('en-US', {
                          weekday: 'short',
                          month: 'short',
                          day: 'numeric',
                      })
                      .toUpperCase()
                : ''
            dateEl.style.display = controls.showDate ? 'block' : 'none'

            cpuValueEl.textContent = sensors.formatted(controls.cpuTempSensor as string)
            gpuValueEl.textContent = sensors.formatted(controls.gpuTempSensor as string)
            loadValueEl.textContent = sensors.formatted(controls.cpuLoadSensor as string)
            ramValueEl.textContent = sensors.formatted(controls.ramSensor as string)
            loadFillEl.style.setProperty('--fill', clamp01(smoothLoad).toFixed(4))
            ramFillEl.style.setProperty('--fill', clamp01(smoothRam).toFixed(4))
            barsEl.style.display = controls.showBars ? 'grid' : 'none'

            const c = ctx.ctx
            c.clearRect(0, 0, W, H)

            const wash = resolveFaceCanvasWash(backdrop, panelColor, panelAlpha)
            if (wash) {
                c.fillStyle = wash
                c.fillRect(0, 0, W, H)
            }

            const ambient = c.createRadialGradient(cx, cy, 18, cx, cy, W * 0.6)
            ambient.addColorStop(0, withAlpha(accent, 0.14))
            ambient.addColorStop(0.55, withAlpha(secondary, 0.06))
            ambient.addColorStop(1, withAlpha(secondary, 0))
            c.fillStyle = ambient
            c.fillRect(0, 0, W, H)
        }
    },
)
