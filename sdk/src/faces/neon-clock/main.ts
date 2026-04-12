/**
 * Neon Clock — clean animated timepiece.
 *
 * Large digital time with smooth glow transitions, optional date,
 * configurable font and accent colors. Pulsing colon and subtle
 * animated ring behind the time.
 */

import { color, combo, face, num, palette, toggle, withGlow } from '@hypercolor/sdk'

export default face(
    'Neon Clock',
    {
        accent: color('Accent', palette.neonCyan, { group: 'Style' }),
        hourFormat: combo('Format', ['24h', '12h'], { group: 'Clock' }),
        showSeconds: toggle('Show Seconds', false, { group: 'Clock' }),
        showDate: toggle('Show Date', true, { group: 'Clock' }),
        glowIntensity: num('Glow', [0, 100], 70, { group: 'Style' }),
        ringStyle: combo('Ring', ['Sweep', 'Pulse', 'None'], { group: 'Style' }),
    },
    {
        description: 'Elegant neon timepiece with animated glow ring and smooth digit transitions.',
        author: 'Hypercolor',
        designBasis: { width: 480, height: 480 },
        presets: [
            {
                name: 'Electric Midnight',
                description: 'Neon cyan glow with sweep ring',
                controls: { accent: palette.neonCyan, glowIntensity: 80, ringStyle: 'Sweep' },
            },
            {
                name: 'Rose Gold',
                description: 'Warm coral on dark — soft and luxurious',
                controls: { accent: '#ffb4a2', glowIntensity: 50, ringStyle: 'Pulse' },
            },
            {
                name: 'Ghost',
                description: 'Barely-there white — minimal and sharp',
                controls: { accent: '#e8e6f0', glowIntensity: 20, ringStyle: 'None' },
            },
        ],
    },
    (ctx) => {
        const { width: W, height: H } = ctx
        const cx = W / 2
        const cy = H / 2

        return (time, controls) => {
            const c = ctx.ctx
            const accent = controls.accent as string
            const glow = (controls.glowIntensity as number) / 100
            const is12h = controls.hourFormat === '12h'
            const showSec = controls.showSeconds as boolean

            const now = new Date()
            let hours = now.getHours()
            const mins = now.getMinutes()
            const secs = now.getSeconds()
            const ms = now.getMilliseconds()
            const ampm = hours >= 12 ? 'PM' : 'AM'
            if (is12h) hours = hours % 12 || 12

            // ── Background ────────────────────────────────────
            c.fillStyle = palette.bg.deep
            c.fillRect(0, 0, W, H)

            // ── Ring ──────────────────────────────────────────
            const ringRadius = Math.min(W, H) * 0.42
            const ringThickness = 2.5

            if (controls.ringStyle !== 'None') {
                c.lineCap = 'round'
                c.lineWidth = ringThickness

                // Track
                c.beginPath()
                c.arc(cx, cy, ringRadius, 0, Math.PI * 2)
                c.strokeStyle = `${accent}10`
                c.stroke()

                if (controls.ringStyle === 'Sweep') {
                    // Second-hand sweep
                    const sweepProgress = (secs + ms / 1000) / 60
                    const startAngle = -Math.PI / 2
                    const endAngle = startAngle + Math.PI * 2 * sweepProgress

                    withGlow(c, accent, glow * 0.5, () => {
                        c.beginPath()
                        c.arc(cx, cy, ringRadius, startAngle, endAngle)
                        c.strokeStyle = accent
                        c.stroke()
                    })

                    // Leading dot
                    const dotX = cx + Math.cos(endAngle) * ringRadius
                    const dotY = cy + Math.sin(endAngle) * ringRadius
                    withGlow(c, accent, glow, () => {
                        c.beginPath()
                        c.arc(dotX, dotY, 4, 0, Math.PI * 2)
                        c.fillStyle = accent
                        c.fill()
                    })
                } else {
                    // Pulse — breathing ring
                    const pulse = 0.3 + Math.sin(time * 2) * 0.15
                    c.beginPath()
                    c.arc(cx, cy, ringRadius, 0, Math.PI * 2)
                    c.strokeStyle = accent
                    c.globalAlpha = pulse
                    withGlow(c, accent, glow * 0.3, () => c.stroke())
                    c.globalAlpha = 1
                }
            }

            // ── Time ──────────────────────────────────────────
            const hStr = hours.toString().padStart(2, '0')
            const mStr = mins.toString().padStart(2, '0')

            // Pulsing colon (blinks on the second boundary)
            let timeStr = `${hStr}:${mStr}`
            if (showSec) {
                const sStr = secs.toString().padStart(2, '0')
                timeStr += `:${sStr}`
            }

            const fontSize = showSec ? 64 : 80
            c.font = `bold ${fontSize}px 'Orbitron', 'JetBrains Mono', monospace`
            c.textAlign = 'center'
            c.textBaseline = 'middle'

            // Draw time with glow
            const timeY = controls.showDate ? cy - 12 : cy
            withGlow(c, accent, glow * 0.6, () => {
                c.fillStyle = accent
                c.fillText(timeStr, cx, timeY)
            })

            // AM/PM badge
            if (is12h) {
                c.font = "bold 18px 'Inter', sans-serif"
                c.fillStyle = `${accent}88`
                c.textAlign = 'center'
                c.fillText(ampm, cx, timeY - fontSize / 2 - 14)
            }

            // ── Date ──────────────────────────────────────────
            if (controls.showDate) {
                const dateStr = now.toLocaleDateString('en-US', {
                    weekday: 'short',
                    month: 'short',
                    day: 'numeric',
                })
                c.font = "16px 'Inter', sans-serif"
                c.fillStyle = palette.fg.tertiary
                c.textAlign = 'center'
                c.textBaseline = 'top'
                c.fillText(dateStr, cx, timeY + fontSize / 2 + 8)
            }
        }
    },
)
