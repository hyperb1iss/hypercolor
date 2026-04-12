/**
 * SilkCircuit HUD — flagship system monitoring face.
 *
 * Animated arc gauges for CPU and GPU, memory bar, load bars, clock with
 * date, and a trailing temperature sparkline. Full SilkCircuit neon
 * aesthetic with configurable accent colors and fonts.
 */

import {
    ValueHistory,
    arcGauge,
    barGauge,
    color,
    colorByValue,
    combo,
    face,
    palette,
    sensor,
    sensorColors,
    sparkline,
    toggle,
} from '@hypercolor/sdk'

export default face(
    'SilkCircuit HUD',
    {
        cpuTempSensor: sensor('CPU Temp Sensor', 'cpu_temp', { group: 'Sensors' }),
        gpuTempSensor: sensor('GPU Temp Sensor', 'gpu_temp', { group: 'Sensors' }),
        cpuLoadSensor: sensor('CPU Load Sensor', 'cpu_load', { group: 'Sensors' }),
        ramSensor: sensor('RAM Sensor', 'ram_used', { group: 'Sensors' }),
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        secondaryAccent: color('Secondary', palette.coral, { group: 'Style' }),
        hourFormat: combo('Clock Format', ['24h', '12h'], { group: 'Clock' }),
        showDate: toggle('Show Date', true, { group: 'Clock' }),
        showSparkline: toggle('Show Sparkline', true, { group: 'Layout' }),
        gaugeGlow: combo('Gauge Glow', ['High', 'Low', 'Off'], { group: 'Style' }),
    },
    {
        description: 'Animated system dashboard — arc gauges, bars, clock, sparkline. Full SilkCircuit neon aesthetic.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'SilkCircuit Dark',
                description: 'Neon cyan + coral on deep black — the signature look',
                controls: { accent: palette.neonCyan, secondaryAccent: palette.coral, gaugeGlow: 'High' },
            },
            {
                name: 'Forge',
                description: 'Amber heat on charcoal — for the overclockers',
                controls: { accent: '#ffb347', secondaryAccent: '#ff6b6b', gaugeGlow: 'Low' },
            },
            {
                name: 'Arctic',
                description: 'Ice blue minimalism — cool and clean',
                controls: { accent: '#7ec8e3', secondaryAccent: '#c8b6ff', gaugeGlow: 'Off' },
            },
        ],
    },
    (ctx) => {
        const { width: W, height: H } = ctx
        const cx = W / 2

        // State
        const cpuTempHistory = new ValueHistory(60)
        const gpuTempHistory = new ValueHistory(60)
        let smoothCpuTemp = 0
        let smoothGpuTemp = 0
        let smoothCpuLoad = 0
        let smoothRam = 0
        let lastHistoryPush = 0

        // Smooth approach helper
        const approach = (current: number, target: number, speed: number): number =>
            current + (target - current) * Math.min(1, speed)

        return (time, controls, sensors) => {
            const c = ctx.ctx
            const accent = controls.accent as string
            const secondary = controls.secondaryAccent as string
            const glow = controls.gaugeGlow === 'High' ? 0.8 : controls.gaugeGlow === 'Low' ? 0.3 : 0

            // Read sensors
            const cpuTemp = sensors.normalized(controls.cpuTempSensor as string)
            const gpuTemp = sensors.normalized(controls.gpuTempSensor as string)
            const cpuLoad = sensors.normalized(controls.cpuLoadSensor as string)
            const ram = sensors.normalized(controls.ramSensor as string)

            // Smooth values for animation
            smoothCpuTemp = approach(smoothCpuTemp, cpuTemp, 0.08)
            smoothGpuTemp = approach(smoothGpuTemp, gpuTemp, 0.08)
            smoothCpuLoad = approach(smoothCpuLoad, cpuLoad, 0.12)
            smoothRam = approach(smoothRam, ram, 0.1)

            // Push to history every ~500ms
            if (time - lastHistoryPush > 0.5) {
                cpuTempHistory.push(cpuTemp)
                gpuTempHistory.push(gpuTemp)
                lastHistoryPush = time
            }

            // ── Background ────────────────────────────────────────
            c.fillStyle = palette.bg.deep
            c.fillRect(0, 0, W, H)

            // Subtle radial vignette
            const vignette = c.createRadialGradient(cx, H / 2, W * 0.2, cx, H / 2, W * 0.7)
            vignette.addColorStop(0, 'transparent')
            vignette.addColorStop(1, 'rgba(0, 0, 0, 0.4)')
            c.fillStyle = vignette
            c.fillRect(0, 0, W, H)

            // ── Clock ─────────────────────────────────────────────
            const now = new Date()
            const is12h = controls.hourFormat === '12h'
            let hours = now.getHours()
            const minutes = now.getMinutes()
            const ampm = hours >= 12 ? 'PM' : 'AM'
            if (is12h) hours = hours % 12 || 12
            const timeStr = `${hours.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}`

            c.font = "bold 52px 'Orbitron', 'JetBrains Mono', monospace"
            c.fillStyle = palette.fg.primary
            c.textAlign = 'center'
            c.textBaseline = 'top'
            c.fillText(timeStr + (is12h ? ` ${ampm}` : ''), cx, 28)

            if (controls.showDate) {
                const dateStr = now.toLocaleDateString('en-US', {
                    month: 'short',
                    day: 'numeric',
                    year: 'numeric',
                })
                c.font = "16px 'Inter', sans-serif"
                c.fillStyle = palette.fg.tertiary
                c.fillText(dateStr, cx, 88)
            }

            // ── Arc Gauges — CPU and GPU temp ─────────────────────
            const gaugeY = 195
            const gaugeR = 72
            const gaugeThickness = 10

            // CPU temp gauge (left)
            const cpuColor = colorByValue(smoothCpuTemp, sensorColors.temperature.gradient)
            arcGauge(c, {
                cx: cx - 90,
                cy: gaugeY,
                radius: gaugeR,
                thickness: gaugeThickness,
                value: smoothCpuTemp,
                fillColor: [accent, cpuColor],
                glow,
            })

            // CPU temp label + value
            c.font = "bold 28px 'JetBrains Mono', monospace"
            c.fillStyle = cpuColor
            c.textAlign = 'center'
            c.textBaseline = 'middle'
            c.fillText(sensors.formatted(controls.cpuTempSensor as string), cx - 90, gaugeY)
            c.font = "13px 'Inter', sans-serif"
            c.fillStyle = palette.fg.secondary
            c.fillText('CPU', cx - 90, gaugeY + 22)

            // GPU temp gauge (right)
            const gpuColor = colorByValue(smoothGpuTemp, sensorColors.temperature.gradient)
            arcGauge(c, {
                cx: cx + 90,
                cy: gaugeY,
                radius: gaugeR,
                thickness: gaugeThickness,
                value: smoothGpuTemp,
                fillColor: [secondary, gpuColor],
                glow,
            })

            // GPU temp label + value
            c.font = "bold 28px 'JetBrains Mono', monospace"
            c.fillStyle = gpuColor
            c.textAlign = 'center'
            c.textBaseline = 'middle'
            c.fillText(sensors.formatted(controls.gpuTempSensor as string), cx + 90, gaugeY)
            c.font = "13px 'Inter', sans-serif"
            c.fillStyle = palette.fg.secondary
            c.fillText('GPU', cx + 90, gaugeY + 22)

            // ── Load Bars ─────────────────────────────────────────
            const barX = 48
            const barW = W - 96
            const barH = 14
            const barY = 305

            // CPU Load bar
            c.font = "12px 'Inter', sans-serif"
            c.fillStyle = palette.fg.secondary
            c.textAlign = 'left'
            c.textBaseline = 'middle'
            c.fillText('CPU', barX, barY - 2)
            c.textAlign = 'right'
            c.fillText(`${Math.round(smoothCpuLoad * 100)}%`, barX + barW, barY - 2)

            barGauge(c, {
                x: barX,
                y: barY + 8,
                width: barW,
                height: barH,
                value: smoothCpuLoad,
                fillColor: [accent, colorByValue(smoothCpuLoad, sensorColors.load.gradient)],
                borderRadius: 7,
                glow: glow * 0.5,
            })

            // RAM bar
            const ramY = barY + 48
            c.font = "12px 'Inter', sans-serif"
            c.fillStyle = palette.fg.secondary
            c.textAlign = 'left'
            c.textBaseline = 'middle'
            c.fillText('RAM', barX, ramY - 2)
            c.textAlign = 'right'
            c.fillText(sensors.formatted(controls.ramSensor as string), barX + barW, ramY - 2)

            barGauge(c, {
                x: barX,
                y: ramY + 8,
                width: barW,
                height: barH,
                value: smoothRam,
                fillColor: sensorColors.memory.gradient,
                borderRadius: 7,
                glow: glow * 0.5,
            })

            // ── Sparkline ─────────────────────────────────────────
            if (controls.showSparkline && cpuTempHistory.length > 2) {
                const sparkY = 410
                const sparkH = 44

                // Subtle separator line
                c.strokeStyle = palette.bg.raised
                c.lineWidth = 1
                c.beginPath()
                c.moveTo(barX, sparkY - 8)
                c.lineTo(barX + barW, sparkY - 8)
                c.stroke()

                sparkline(c, {
                    x: barX,
                    y: sparkY,
                    width: barW / 2 - 8,
                    height: sparkH,
                    values: cpuTempHistory.values(),
                    range: [0, 1],
                    color: accent,
                    lineWidth: 1.5,
                })

                sparkline(c, {
                    x: cx + 8,
                    y: sparkY,
                    width: barW / 2 - 8,
                    height: sparkH,
                    values: gpuTempHistory.values(),
                    range: [0, 1],
                    color: secondary,
                    lineWidth: 1.5,
                })

                // Sparkline labels
                c.font = "10px 'Inter', sans-serif"
                c.fillStyle = palette.fg.tertiary
                c.textAlign = 'left'
                c.textBaseline = 'bottom'
                c.fillText('CPU temp', barX, sparkY - 1)
                c.fillText('GPU temp', cx + 8, sparkY - 1)
            }
        }
    },
)
