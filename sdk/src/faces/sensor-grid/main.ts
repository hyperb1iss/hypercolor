/**
 * Sensor Grid — multi-sensor dashboard.
 *
 * 2x2 grid of compact ring gauges, each bound to a configurable sensor.
 * Adapts layout to display resolution. Clean, information-dense, and
 * endlessly useful.
 */

import {
    color,
    colorByValue,
    combo,
    face,
    palette,
    ringGauge,
    sensor,
    sensorColors,
} from '@hypercolor/sdk'

export default face(
    'Sensor Grid',
    {
        sensor1: sensor('Top Left', 'cpu_temp', { group: 'Sensors' }),
        sensor2: sensor('Top Right', 'gpu_temp', { group: 'Sensors' }),
        sensor3: sensor('Bottom Left', 'cpu_load', { group: 'Sensors' }),
        sensor4: sensor('Bottom Right', 'ram_used', { group: 'Sensors' }),
        colorMode: combo('Colors', ['Auto', 'Accent'], { group: 'Style' }),
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        style: combo('Style', ['Rings', 'Minimal'], { group: 'Style' }),
    },
    {
        description: '2x2 grid of compact sensor gauges — see everything at a glance.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'System Vitals',
                description: 'CPU temp, GPU temp, CPU load, RAM',
                controls: {
                    sensor1: 'cpu_temp',
                    sensor2: 'gpu_temp',
                    sensor3: 'cpu_load',
                    sensor4: 'ram_used',
                    colorMode: 'Auto',
                },
            },
            {
                name: 'Thermal Focus',
                description: 'All temperature sensors',
                controls: {
                    sensor1: 'cpu_temp',
                    sensor2: 'gpu_temp',
                    sensor3: 'cpu_temp',
                    sensor4: 'gpu_temp',
                    colorMode: 'Auto',
                },
            },
        ],
    },
    (ctx) => {
        const { width: W, height: H } = ctx

        // 2x2 grid positions
        const pad = 36
        const cellW = (W - pad * 3) / 2
        const cellH = (H - pad * 3) / 2
        const cells = [
            { cx: pad + cellW / 2, cy: pad + cellH / 2 },
            { cx: pad * 2 + cellW + cellW / 2, cy: pad + cellH / 2 },
            { cx: pad + cellW / 2, cy: pad * 2 + cellH + cellH / 2 },
            { cx: pad * 2 + cellW + cellW / 2, cy: pad * 2 + cellH + cellH / 2 },
        ]

        const sensorKeys = ['sensor1', 'sensor2', 'sensor3', 'sensor4'] as const
        const smoothValues = [0, 0, 0, 0]

        const approach = (current: number, target: number): number =>
            current + (target - current) * 0.08

        return (_time, controls, sensors) => {
            const c = ctx.ctx
            const isAuto = controls.colorMode === 'Auto'
            const accentColor = controls.accent as string
            const isMinimal = controls.style === 'Minimal'

            c.fillStyle = palette.bg.deep
            c.fillRect(0, 0, W, H)

            // Subtle grid lines
            c.strokeStyle = palette.bg.overlay
            c.lineWidth = 1
            c.beginPath()
            c.moveTo(W / 2, pad * 0.5)
            c.lineTo(W / 2, H - pad * 0.5)
            c.moveTo(pad * 0.5, H / 2)
            c.lineTo(W - pad * 0.5, H / 2)
            c.stroke()

            for (let i = 0; i < 4; i++) {
                const sensorLabel = controls[sensorKeys[i]] as string
                const raw = sensors.normalized(sensorLabel)
                smoothValues[i] = approach(smoothValues[i], raw)
                const val = smoothValues[i]

                const reading = sensors.read(sensorLabel)
                const formatted = sensors.formatted(sensorLabel)
                const label = sensorLabel.replace(/_/g, ' ').toUpperCase()

                // Determine color
                let ringColor: string
                if (isAuto) {
                    const unit = reading?.unit ?? '%'
                    if (unit === '°C' || unit === '°F') {
                        ringColor = colorByValue(val, sensorColors.temperature.gradient)
                    } else if (unit === 'MB') {
                        ringColor = colorByValue(val, sensorColors.memory.gradient)
                    } else {
                        ringColor = colorByValue(val, sensorColors.load.gradient)
                    }
                } else {
                    ringColor = accentColor
                }

                const { cx: cellCx, cy: cellCy } = cells[i]
                const ringR = Math.min(cellW, cellH) * (isMinimal ? 0.32 : 0.36)

                if (isMinimal) {
                    // Minimal — just value text + tiny ring
                    ringGauge(c, {
                        cx: cellCx,
                        cy: cellCy - 8,
                        radius: ringR,
                        thickness: 3,
                        value: val,
                        color: ringColor,
                        valueText: formatted,
                        valueFont: `bold ${Math.round(ringR * 0.55)}px 'JetBrains Mono', monospace`,
                        valueColor: ringColor,
                        label,
                        labelColor: palette.fg.tertiary,
                        labelFont: `${Math.round(ringR * 0.22)}px 'Inter', sans-serif`,
                    })
                } else {
                    // Full rings with glow
                    ringGauge(c, {
                        cx: cellCx,
                        cy: cellCy - 4,
                        radius: ringR,
                        thickness: Math.max(4, ringR * 0.1),
                        value: val,
                        color: ringColor,
                        valueText: formatted,
                        valueFont: `bold ${Math.round(ringR * 0.5)}px 'JetBrains Mono', monospace`,
                        valueColor: ringColor,
                        label,
                        labelColor: palette.fg.secondary,
                        labelFont: `${Math.round(ringR * 0.2)}px 'Inter', sans-serif`,
                        glow: 0.4,
                    })
                }
            }
        }
    },
)
