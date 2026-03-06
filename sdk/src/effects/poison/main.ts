import { canvas, color, num } from '@hypercolor/sdk'

interface RGB {
    r: number
    g: number
    b: number
}

interface PoisonOrb {
    seed: number
    lane: number
    direction: number
    size: number
    speed: number
    drift: number
    wobble: number
    colorMix: number
    offset: number
    pulse: number
}

const DEFAULT_BG = '#130032'
const DEFAULT_COLOR_1 = '#6000fc'
const DEFAULT_COLOR_2 = '#b300ff'
const DEFAULT_COLOR_3 = '#8a42ff'

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function fract(value: number): number {
    return value - Math.floor(value)
}

function hexToRgb(hex: string): RGB {
    const normalized = hex.replace('#', '')
    const full = normalized.length === 3
        ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
        : normalized
    const parsed = parseInt(full, 16)
    if (Number.isNaN(parsed)) return { r: 255, g: 0, b: 255 }
    return {
        r: (parsed >> 16) & 255,
        g: (parsed >> 8) & 255,
        b: parsed & 255,
    }
}

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function mixRgb(a: RGB, b: RGB, amount: number): RGB {
    const t = clamp(amount, 0, 1)
    return {
        r: Math.round(a.r + (b.r - a.r) * t),
        g: Math.round(a.g + (b.g - a.g) * t),
        b: Math.round(a.b + (b.b - a.b) * t),
    }
}

function rgbToHsl(color: RGB): { h: number; s: number; l: number } {
    const r = color.r / 255
    const g = color.g / 255
    const b = color.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    const l = (max + min) / 2

    if (delta === 0) return { h: 0, s: 0, l }

    const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let h = 0
    if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / delta + 2
    else h = (r - g) / delta + 4

    return { h: h * 60, s, l }
}

function hslToRgb(h: number, sPercent: number, lPercent: number): RGB {
    const hue = ((h % 360) + 360) % 360
    const s = clamp(sPercent, 0, 100) / 100
    const l = clamp(lPercent, 0, 100) / 100
    const c = (1 - Math.abs(2 * l - 1)) * s
    const hp = hue / 60
    const x = c * (1 - Math.abs((hp % 2) - 1))

    let r = 0
    let g = 0
    let b = 0
    if (hp < 1) [r, g, b] = [c, x, 0]
    else if (hp < 2) [r, g, b] = [x, c, 0]
    else if (hp < 3) [r, g, b] = [0, c, x]
    else if (hp < 4) [r, g, b] = [0, x, c]
    else if (hp < 5) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    const m = l - c / 2
    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    }
}

function enrichRgb(color: RGB, saturationBoost: number, lightnessOffset = 0): RGB {
    const { h, s, l } = rgbToHsl(color)
    return hslToRgb(
        h,
        clamp((s + saturationBoost) * 100, 0, 100),
        clamp((l + lightnessOffset) * 100, 0, 100),
    )
}

function buildPalette(color1: string, color2: string, color3: string): RGB[] {
    return [
        enrichRgb(hexToRgb(color1), 0.18, -0.05),
        enrichRgb(hexToRgb(color2), 0.20, -0.03),
        enrichRgb(hexToRgb(color3), 0.14, -0.04),
    ]
}

function paletteColor(phase: number, palette: RGB[]): RGB {
    const t = fract(phase)
    if (t < 1 / 3) return mixRgb(palette[0], palette[1], t * 3)
    if (t < 2 / 3) return mixRgb(palette[1], palette[2], (t - 1 / 3) * 3)
    return mixRgb(palette[2], palette[0], (t - 2 / 3) * 3)
}

function createOrbs(count: number): PoisonOrb[] {
    return Array.from({ length: count }, (_, index) => ({
        seed: Math.random(),
        lane: Math.random(),
        direction: index % 2 === 0 ? 1 : -1,
        size: 0.45 + Math.random() * 0.95,
        speed: 0.55 + Math.random() * 1.2,
        drift: 0.15 + Math.random() * 0.85,
        wobble: Math.random() * Math.PI * 2,
        colorMix: Math.random(),
        offset: Math.random(),
        pulse: Math.random() * Math.PI * 2,
    }))
}

export default canvas.stateful('Poison', {
    bgColor:  color('Background Color', DEFAULT_BG),
    color1:   color('Color 1', DEFAULT_COLOR_1),
    color2:   color('Color 2', DEFAULT_COLOR_2),
    color3:   color('Color 3', DEFAULT_COLOR_3),
    speedRaw: num('Speed', [0, 100], 24),
}, () => {
    let orbs = createOrbs(26)

    function drawBackdrop(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        background: string,
        palette: RGB[],
        speed: number,
    ): void {
        ctx.fillStyle = background
        ctx.fillRect(0, 0, width, height)

        const centerX = width * (0.5 + Math.sin(time * 0.16) * 0.08)
        const centerY = height * (0.5 + Math.cos(time * 0.14) * 0.06)
        const radius = Math.max(width, height) * 0.82
        const bloom = ctx.createRadialGradient(centerX, centerY, 0, centerX, centerY, radius)
        bloom.addColorStop(0, rgba(palette[1], 0.18 + speed * 0.10))
        bloom.addColorStop(0.45, rgba(palette[2], 0.10 + speed * 0.08))
        bloom.addColorStop(1, 'rgba(0, 0, 0, 0)')
        ctx.fillStyle = bloom
        ctx.fillRect(0, 0, width, height)

        const veil = ctx.createLinearGradient(0, 0, width, height)
        veil.addColorStop(0, rgba(palette[0], 0.08))
        veil.addColorStop(0.5, rgba(palette[1], 0.05))
        veil.addColorStop(1, rgba(palette[2], 0.08))
        ctx.fillStyle = veil
        ctx.fillRect(0, 0, width, height)
    }

    function drawCurrentBand(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        palette: RGB[],
        speed: number,
    ): void {
        ctx.save()
        ctx.globalCompositeOperation = 'screen'

        for (let band = 0; band < 3; band++) {
            const color = palette[(band + 1) % palette.length]
            const phase = time * (0.35 + speed * 0.55) + band * 1.7
            ctx.strokeStyle = rgba(color, 0.18 + speed * 0.10)
            ctx.lineWidth = 16 + band * 10
            ctx.lineCap = 'round'
            ctx.beginPath()

            for (let step = 0; step <= 18; step++) {
                const t = step / 18
                const x = t * width
                const yBase = height * (0.22 + band * 0.24)
                const y = yBase
                    + Math.sin(t * 6.4 + phase) * (12 + band * 4)
                    + Math.cos(t * 3.1 - phase * 0.9) * (8 + band * 3)
                if (step === 0) ctx.moveTo(x, y)
                else ctx.lineTo(x, y)
            }

            ctx.stroke()
        }

        ctx.restore()
    }

    function drawOrb(
        ctx: CanvasRenderingContext2D,
        orb: PoisonOrb,
        width: number,
        height: number,
        time: number,
        palette: RGB[],
        speed: number,
    ): void {
        const flow = time * (0.24 + speed * 1.9) * orb.speed
        let progress = fract(orb.offset + flow * 0.11)
        if (orb.direction < 0) progress = 1 - progress

        const laneX = width * (0.10 + orb.lane * 0.80)
        const driftX = Math.sin(time * (0.7 + orb.drift) + orb.wobble) * width * (0.02 + orb.drift * 0.05)
        const x = laneX + driftX
        const y = progress * height
        const pulse = 0.72 + 0.28 * Math.sin(time * (2.1 + orb.speed) + orb.pulse)
        const baseRadius = (8 + orb.size * 18) * (0.7 + pulse * 0.55)
        const radius = baseRadius * (0.72 + Math.sin(progress * Math.PI) * 0.66)
        const color = paletteColor(orb.colorMix + time * 0.05 + progress * 0.22, palette)
        const glow = enrichRgb(color, 0.10, 0.03)
        const core = enrichRgb(color, 0.18, 0.08)

        ctx.fillStyle = rgba(glow, 0.12 + speed * 0.10)
        ctx.beginPath()
        ctx.arc(x, y, radius * 2.3, 0, Math.PI * 2)
        ctx.fill()

        ctx.strokeStyle = rgba(core, 0.34 + speed * 0.14)
        ctx.lineWidth = Math.max(1.2, radius * 0.22)
        ctx.beginPath()
        ctx.arc(x, y, radius, 0, Math.PI * 2)
        ctx.stroke()

        ctx.fillStyle = rgba(color, 0.20 + speed * 0.12)
        ctx.beginPath()
        ctx.arc(x, y, radius * 0.72, 0, Math.PI * 2)
        ctx.fill()

        ctx.fillStyle = rgba(core, 0.30 + speed * 0.08)
        ctx.beginPath()
        ctx.arc(x - radius * 0.18, y - radius * 0.18, radius * 0.24, 0, Math.PI * 2)
        ctx.fill()
    }

    return (ctx, time, controls) => {
        const width = ctx.canvas.width
        const height = ctx.canvas.height
        const background = controls.bgColor as string
        const speed = clamp((controls.speedRaw as number) / 100, 0, 1)
        const palette = buildPalette(
            controls.color1 as string,
            controls.color2 as string,
            controls.color3 as string,
        )

        if (orbs.length !== 26) {
            orbs = createOrbs(26)
        }

        drawBackdrop(ctx, width, height, time, background, palette, speed)

        ctx.save()
        ctx.fillStyle = rgba(hexToRgb(background), 0.20 - speed * 0.08)
        ctx.fillRect(0, 0, width, height)
        ctx.restore()

        drawCurrentBand(ctx, width, height, time, palette, speed)

        ctx.save()
        ctx.globalCompositeOperation = 'screen'
        for (const orb of orbs) {
            drawOrb(ctx, orb, width, height, time, palette, speed)
        }
        ctx.restore()
    }
}, {
    description: 'A rolling brew of poison with luminous bubbling rings and drifting neon currents',
    author: 'Hypercolor',
})
