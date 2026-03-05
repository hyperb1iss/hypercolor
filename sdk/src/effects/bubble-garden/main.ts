import { canvas } from '@hypercolor/sdk'

const COLOR_MODES = ['Single Color', 'Rainbow', 'Color Cycle']

interface Bubble { x: number; y: number; vx: number; vy: number; baseSize: number; alpha: number }

// ── Helpers ──────────────────────────────────────────────────────────────

function rand(min: number, max: number) {
    return Math.floor(Math.random() * (max - min + 1)) + min
}

function randomVelocity(): number {
    const v = rand(-10, 10) / 10
    return Math.abs(v) < 0.001 ? (Math.random() < 0.5 ? -1 : 1) : v
}

function hexToRgb(hex: string) {
    const norm = hex.replace('#', '')
    const full = norm.length === 3
        ? `${norm[0]}${norm[0]}${norm[1]}${norm[1]}${norm[2]}${norm[2]}`
        : norm
    const n = parseInt(full, 16)
    return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 }
}

function hexToRgba(hex: string, alpha: number): string {
    const c = hexToRgb(hex)
    return `rgba(${c.r},${c.g},${c.b},${Math.max(0, Math.min(1, alpha)).toFixed(3)})`
}

function hslToHex(h: number, s: number, l: number): string {
    h = ((h % 360) + 360) % 360
    s /= 100
    l /= 100
    const c = (1 - Math.abs(2 * l - 1)) * s
    const x = c * (1 - Math.abs(((h / 60) % 2) - 1))
    const m = l - c / 2
    let r = 0, g = 0, b = 0
    if (h < 60) [r, g, b] = [c, x, 0]
    else if (h < 120) [r, g, b] = [x, c, 0]
    else if (h < 180) [r, g, b] = [0, c, x]
    else if (h < 240) [r, g, b] = [0, x, c]
    else if (h < 300) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]
    const toHex = (v: number) => Math.round((v + m) * 255).toString(16).padStart(2, '0')
    return `#${toHex(r)}${toHex(g)}${toHex(b)}`
}

function createBubbles(count: number, width: number, height: number): Bubble[] {
    const bubbles: Bubble[] = []
    for (let i = 0; i < count; i++) {
        const radius = rand(10, 20)
        bubbles.push({
            x: rand(radius, Math.max(radius, width - radius)),
            y: rand(radius, Math.max(radius, height - radius)),
            vx: randomVelocity(),
            vy: randomVelocity(),
            baseSize: radius,
            alpha: 0.5 + (i / Math.max(1, count - 1)) * 0.4,
        })
    }
    return bubbles
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful('Bubble Garden', {
    colorMode: COLOR_MODES,
    bgColor:   '#000000',
    color:     '#ff0066',
    speed:     [0, 100, 10],
    size:      [1, 10, 5],
    count:     [10, 120, 50],
}, () => {
    let bubbles = createBubbles(50, 320, 200)
    let prevCount = 50
    let prevSpeed = 10
    let hue = 0

    return (ctx, _time, c) => {
        const count = Math.max(10, Math.floor(c.count as number))
        const speed = c.speed as number
        const size = c.size as number
        const colorMode = c.colorMode as string
        const bgColor = c.bgColor as string
        const color = c.color as string
        const width = ctx.canvas.width
        const height = ctx.canvas.height

        if (count !== prevCount) {
            bubbles = createBubbles(count, width, height)
            prevCount = count
        } else if (speed !== prevSpeed) {
            for (const b of bubbles) { b.vx = randomVelocity(); b.vy = randomVelocity() }
            prevSpeed = speed
        }

        ctx.fillStyle = bgColor
        ctx.fillRect(0, 0, width, height)

        const speedScale = Math.max(0, speed) / 10
        const sizeScale = Math.max(0.2, size / 5)
        hue = (hue + 1) % 360

        for (const b of bubbles) {
            const radius = b.baseSize * sizeScale

            if (speedScale > 0) {
                b.x += b.vx * speedScale
                b.y += b.vy * speedScale
                if (b.x + radius >= width)  { b.x = width - radius; b.vx = -Math.abs(b.vx) }
                else if (b.x - radius <= 0) { b.x = radius; b.vx = Math.abs(b.vx) }
                if (b.y + radius >= height) { b.y = height - radius; b.vy = -Math.abs(b.vy) }
                else if (b.y - radius <= 0) { b.y = radius; b.vy = Math.abs(b.vy) }
            }

            let fill = color
            if (colorMode === 'Color Cycle') fill = hslToHex(hue, 100, 50)
            else if (colorMode === 'Rainbow') fill = hslToHex((b.x / Math.max(width, 1)) * 360, 100, 50)

            ctx.fillStyle = hexToRgba(fill, b.alpha)
            ctx.beginPath()
            ctx.arc(b.x, b.y, radius, 0, Math.PI * 2)
            ctx.fill()

            ctx.strokeStyle = hexToRgba('#ffffff', 0.22)
            ctx.lineWidth = 1
            ctx.beginPath()
            ctx.arc(b.x, b.y, Math.max(1, radius - 0.5), 0, Math.PI * 2)
            ctx.stroke()
        }
    }
}, {
    description: 'Community-style bouncing bubbles with crisp rendering and simple controls',
})
