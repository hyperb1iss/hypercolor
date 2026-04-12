/**
 * Pulse Temp — single-sensor hero display.
 *
 * Dramatic animated ring gauge with large centered readout, color-shifted
 * by value (cool → warm → hot), trailing sparkline, and subtle pulsing
 * glow tied to the current temperature.
 */

import {
    ValueHistory,
    color,
    colorByValue,
    combo,
    face,
    num,
    palette,
    ringGauge,
    sensor,
    sensorColors,
    sparkline,
    toggle,
    withGlow,
} from '@hypercolor/sdk'

export default face(
    'Pulse Temp',
    {
        targetSensor: sensor('Sensor', 'cpu_temp', { group: 'Data' }),
        colorScheme: combo('Color Scheme', ['Temperature', 'Load', 'Memory', 'Custom'], { group: 'Style' }),
        customColor: color('Custom Color', palette.neonCyan, { group: 'Style' }),
        glowIntensity: num('Glow', [0, 100], 80, { group: 'Style' }),
        showSparkline: toggle('Sparkline', true, { group: 'Layout' }),
        showLabel: toggle('Label', true, { group: 'Layout' }),
        pulseWithValue: toggle('Pulse Animation', true, { group: 'Style' }),
    },
    {
        description: 'Single-sensor hero display with animated ring gauge, value-driven color shifts, and trailing sparkline.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'CPU Thermal',
                description: 'Temperature-mapped colors from cool cyan to hot red',
                controls: { targetSensor: 'cpu_temp', colorScheme: 'Temperature', glowIntensity: 80 },
            },
            {
                name: 'GPU Watch',
                description: 'GPU temperature with high glow',
                controls: { targetSensor: 'gpu_temp', colorScheme: 'Temperature', glowIntensity: 100 },
            },
            {
                name: 'System Load',
                description: 'CPU load percentage with green-to-pink gradient',
                controls: { targetSensor: 'cpu_load', colorScheme: 'Load', glowIntensity: 60 },
            },
        ],
    },
    (ctx) => {
        const { width: W, height: H } = ctx
        const cx = W / 2

        const history = new ValueHistory(80)
        let smoothValue = 0
        let lastPush = 0

        const approach = (current: number, target: number, speed: number): number =>
            current + (target - current) * Math.min(1, speed)

        return (time, controls, sensors) => {
            const c = ctx.ctx
            const glow = (controls.glowIntensity as number) / 100
            const raw = sensors.normalized(controls.targetSensor as string)

            smoothValue = approach(smoothValue, raw, 0.06)

            if (time - lastPush > 0.3) {
                history.push(raw)
                lastPush = time
            }

            // Color based on scheme
            const scheme = controls.colorScheme as string
            let valueColor: string
            if (scheme === 'Temperature') {
                valueColor = colorByValue(smoothValue, sensorColors.temperature.gradient)
            } else if (scheme === 'Load') {
                valueColor = colorByValue(smoothValue, sensorColors.load.gradient)
            } else if (scheme === 'Memory') {
                valueColor = colorByValue(smoothValue, sensorColors.memory.gradient)
            } else {
                valueColor = controls.customColor as string
            }

            // ── Background ────────────────────────────────────
            c.fillStyle = palette.bg.deep
            c.fillRect(0, 0, W, H)

            // Ambient glow from the ring
            if (glow > 0.2) {
                const ambientGrad = c.createRadialGradient(cx, H * 0.4, 10, cx, H * 0.4, W * 0.45)
                ambientGrad.addColorStop(0, `${valueColor}08`)
                ambientGrad.addColorStop(0.5, `${valueColor}04`)
                ambientGrad.addColorStop(1, 'transparent')
                c.fillStyle = ambientGrad
                c.fillRect(0, 0, W, H)
            }

            // ── Ring Gauge ────────────────────────────────────
            const ringCy = controls.showSparkline ? H * 0.38 : H * 0.45
            const ringR = Math.min(W, H) * 0.28
            const ringThickness = Math.max(6, ringR * 0.09)

            // Pulse animation — subtle breathing tied to value
            const pulseScale = controls.pulseWithValue
                ? 1 + Math.sin(time * (1.5 + smoothValue * 2)) * 0.008 * glow
                : 1

            c.save()
            c.translate(cx, ringCy)
            c.scale(pulseScale, pulseScale)
            c.translate(-cx, -ringCy)

            ringGauge(c, {
                cx,
                cy: ringCy,
                radius: ringR,
                thickness: ringThickness,
                value: smoothValue,
                color: valueColor,
                trackColor: 'rgba(255, 255, 255, 0.04)',
                valueText: sensors.formatted(controls.targetSensor as string),
                valueFont: `bold ${Math.round(ringR * 0.5)}px 'JetBrains Mono', monospace`,
                valueColor,
                label: controls.showLabel ? (controls.targetSensor as string).replace(/_/g, ' ').toUpperCase() : undefined,
                labelColor: palette.fg.tertiary,
                glow: glow * 0.7,
            })

            c.restore()

            // ── Sparkline ─────────────────────────────────────
            if (controls.showSparkline && history.length > 2) {
                const sparkMargin = 48
                const sparkW = W - sparkMargin * 2
                const sparkH = 60
                const sparkY = H - sparkH - 40

                // Subtle separator
                c.strokeStyle = palette.bg.raised
                c.lineWidth = 1
                c.beginPath()
                c.moveTo(sparkMargin, sparkY - 12)
                c.lineTo(sparkMargin + sparkW, sparkY - 12)
                c.stroke()

                sparkline(c, {
                    x: sparkMargin,
                    y: sparkY,
                    width: sparkW,
                    height: sparkH,
                    values: history.values(),
                    range: [0, 1],
                    color: valueColor,
                    lineWidth: 2,
                    fillOpacity: 0.1,
                })

                // Current value dot at the end of the sparkline
                if (history.length > 0) {
                    const dotX = sparkMargin + sparkW
                    const dotY = sparkY + sparkH - smoothValue * sparkH

                    withGlow(c, valueColor, glow * 0.5, () => {
                        c.beginPath()
                        c.arc(dotX, dotY, 3.5, 0, Math.PI * 2)
                        c.fillStyle = valueColor
                        c.fill()
                    })
                }
            }
        }
    },
)
