import { canvas, color, num } from '@hypercolor/sdk'

interface RGB {
    r: number
    g: number
    b: number
}

interface OrbSeed {
    lane: number
    offset: number
    drift: number
    radius: number
    phase: number
    direction: number
    paletteIndex: number
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function fract(value: number): number {
    return value - Math.floor(value)
}

function hash(value: number): number {
    const s = Math.sin(value * 91.73 + 12.91) * 43758.5453123
    return s - Math.floor(s)
}

function hexToRgb(hex: string): RGB {
    const normalized = hex.trim().replace('#', '')
    const expanded = normalized.length === 3
        ? normalized.split('').map((char) => `${char}${char}`).join('')
        : normalized

    if (!/^[0-9a-fA-F]{6}$/.test(expanded)) {
        return { r: 255, g: 255, b: 255 }
    }

    const value = Number.parseInt(expanded, 16)
    return {
        r: (value >> 16) & 255,
        g: (value >> 8) & 255,
        b: value & 255,
    }
}

function rgbToHsl(color: RGB): { h: number; s: number; l: number } {
    const r = color.r / 255
    const g = color.g / 255
    const b = color.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    const l = (max + min) * 0.5

    if (delta === 0) return { h: 0, s: 0, l }

    const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let h = 0
    if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / delta + 2
    else h = (r - g) / delta + 4

    return { h: h * 60, s, l }
}

function hslToRgb(h: number, s: number, l: number): RGB {
    const hue = ((h % 360) + 360) % 360
    const sat = clamp(s, 0, 1)
    const light = clamp(l, 0, 1)
    const c = (1 - Math.abs(2 * light - 1)) * sat
    const x = c * (1 - Math.abs(((hue / 60) % 2) - 1))
    const m = light - c * 0.5

    let r = 0
    let g = 0
    let b = 0

    if (hue < 60) [r, g, b] = [c, x, 0]
    else if (hue < 120) [r, g, b] = [x, c, 0]
    else if (hue < 180) [r, g, b] = [0, c, x]
    else if (hue < 240) [r, g, b] = [0, x, c]
    else if (hue < 300) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    }
}

function mixRgb(a: RGB, b: RGB, t: number): RGB {
    const ratio = clamp(t, 0, 1)
    return {
        r: Math.round(a.r + (b.r - a.r) * ratio),
        g: Math.round(a.g + (b.g - a.g) * ratio),
        b: Math.round(a.b + (b.b - a.b) * ratio),
    }
}

function enrichRgb(color: RGB, saturationBoost: number, lightnessOffset = 0): RGB {
    const hsl = rgbToHsl(color)
    return hslToRgb(
        hsl.h,
        clamp(hsl.s + saturationBoost, 0, 1),
        clamp(hsl.l + lightnessOffset, 0, 1),
    )
}

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

export default canvas.stateful('Poisonous', {
    bgColor:  color('Background Color', '#130032'),
    color1:   color('Color 1', '#6000fc'),
    color2:   color('Color 2', '#b300ff'),
    color3:   color('Color 3', '#8a42ff'),
    speedRaw: num('Speed', [0, 100], 10),
}, () => {
    let seeds: OrbSeed[] = []
    let lastWidth = 0
    let lastHeight = 0

    function seedOrbs(width: number, height: number): void {
        if (width === lastWidth && height === lastHeight) return
        lastWidth = width
        lastHeight = height

        seeds = Array.from({ length: 22 }, (_, index) => ({
            lane: 0.1 + hash(index * 1.13 + 0.9) * 0.8,
            offset: hash(index * 2.71 + 7.2),
            drift: hash(index * 3.91 + 4.8),
            radius: 0.75 + hash(index * 5.17 + 2.1) * 1.2,
            phase: hash(index * 6.13 + 1.7) * Math.PI * 2,
            direction: index % 2 === 0 ? 1 : -1,
            paletteIndex: index % 3,
        }))
    }

    function drawBackdrop(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        bg: RGB,
        palette: RGB[],
        speedMix: number,
    ): void {
        const top = enrichRgb(bg, 0.08, -0.12)
        const bottom = enrichRgb(bg, 0.04, -0.02)
        const gradient = ctx.createLinearGradient(0, 0, 0, height)
        gradient.addColorStop(0, rgba(top, 0.24 + speedMix * 0.1))
        gradient.addColorStop(1, rgba(bottom, 0.24 + speedMix * 0.1))
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, width, height)

        const bloom = ctx.createRadialGradient(
            width * 0.52,
            height * 0.52,
            0,
            width * 0.52,
            height * 0.52,
            Math.max(width, height) * 0.9,
        )
        bloom.addColorStop(0, rgba(mixRgb(palette[0], palette[1], 0.42), 0.18))
        bloom.addColorStop(0.55, rgba(palette[2], 0.08))
        bloom.addColorStop(1, 'rgba(0, 0, 0, 0)')
        ctx.fillStyle = bloom
        ctx.fillRect(0, 0, width, height)
    }

    function drawBands(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        speedMix: number,
        palette: RGB[],
    ): void {
        ctx.save()
        ctx.globalCompositeOperation = 'screen'

        for (let i = 0; i < 3; i++) {
            const color = palette[i]
            const centerY = height * (0.2 + i * 0.3)
            const amplitude = 12 + i * 6 + speedMix * 10

            const gradient = ctx.createLinearGradient(0, centerY - amplitude, 0, centerY + amplitude)
            gradient.addColorStop(0, rgba(color, 0))
            gradient.addColorStop(0.5, rgba(color, 0.12 + speedMix * 0.08))
            gradient.addColorStop(1, rgba(color, 0))
            ctx.fillStyle = gradient

            ctx.beginPath()
            ctx.moveTo(0, centerY)
            for (let x = 0; x <= width; x += 18) {
                const xf = x / Math.max(width, 1)
                const y = centerY
                    + Math.sin(xf * Math.PI * (2.8 + i * 0.7) + time * (0.8 + speedMix * 1.8) + i) * amplitude
                    + Math.cos(xf * Math.PI * 6.2 - time * (0.46 + i * 0.12)) * amplitude * 0.22
                ctx.lineTo(x, y)
            }
            ctx.lineTo(width, height + 20)
            ctx.lineTo(0, height + 20)
            ctx.closePath()
            ctx.fill()
        }

        ctx.restore()
    }

    function drawOrbs(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        speedMix: number,
        palette: RGB[],
    ): void {
        ctx.save()
        ctx.globalCompositeOperation = 'screen'

        for (const seed of seeds) {
            const travel = fract(seed.offset + time * (0.04 + speedMix * 0.22) * seed.direction)
            const y = seed.direction > 0
                ? -height * 0.16 + travel * height * 1.34
                : height * 1.16 - travel * height * 1.34
            const x = width * seed.lane
                + Math.sin(time * (1.2 + seed.drift) + seed.phase) * (18 + seed.drift * 16)
                + Math.cos(time * (0.6 + seed.drift * 0.5) + seed.phase * 0.8) * 8

            const color = palette[seed.paletteIndex]
            const pulse = 0.72 + 0.28 * Math.sin(time * (2.4 + seed.drift) + seed.phase)
            const radius = (14 + seed.radius * 20) * pulse
            const glowRadius = radius * (1.8 + speedMix * 0.5)
            const ringRadius = radius * (1.12 + 0.08 * Math.sin(time * 2.8 + seed.phase))

            const glow = ctx.createRadialGradient(x, y, 0, x, y, glowRadius)
            glow.addColorStop(0, rgba(color, 0.32))
            glow.addColorStop(0.4, rgba(color, 0.16))
            glow.addColorStop(1, 'rgba(0, 0, 0, 0)')
            ctx.fillStyle = glow
            ctx.beginPath()
            ctx.arc(x, y, glowRadius, 0, Math.PI * 2)
            ctx.fill()

            ctx.strokeStyle = rgba(enrichRgb(color, 0.16, 0.04), 0.8)
            ctx.lineWidth = Math.max(2, radius * 0.12)
            ctx.beginPath()
            ctx.arc(x, y, ringRadius, 0, Math.PI * 2)
            ctx.stroke()

            ctx.fillStyle = rgba(enrichRgb(color, 0.18, 0.02), 0.54)
            ctx.beginPath()
            ctx.arc(x, y, radius * 0.84, 0, Math.PI * 2)
            ctx.fill()

            ctx.fillStyle = rgba(mixRgb(color, { r: 255, g: 255, b: 255 }, 0.2), 0.36)
            ctx.beginPath()
            ctx.arc(x - radius * 0.2, y - radius * 0.18, radius * 0.26, 0, Math.PI * 2)
            ctx.fill()
        }

        ctx.restore()
    }

    return (ctx, time, controls) => {
        const width = ctx.canvas.width
        const height = ctx.canvas.height
        seedOrbs(width, height)

        const speedRaw = controls.speedRaw as number
        const speedMix = speedRaw / 100
        const bg = enrichRgb(hexToRgb(controls.bgColor as string), 0.08, -0.14)
        const palette = [
            enrichRgb(hexToRgb(controls.color1 as string), 0.2, -0.02),
            enrichRgb(hexToRgb(controls.color2 as string), 0.24, -0.01),
            enrichRgb(hexToRgb(controls.color3 as string), 0.18, 0.02),
        ]

        ctx.fillStyle = rgba(bg, speedRaw > 0 ? 0.22 : 1)
        ctx.fillRect(0, 0, width, height)

        drawBackdrop(ctx, width, height, bg, palette, speedMix)
        drawBands(ctx, width, height, time, speedMix, palette)
        drawOrbs(ctx, width, height, time, speedMix, palette)
    }
}, {
    description: 'A rolling brew of neon poison with layered vapor bands and glowing toxic orbs',
    author: 'Hypercolor',
})
