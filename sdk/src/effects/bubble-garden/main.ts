import { canvas } from '@hypercolor/sdk'

const COLOR_MODES = ['Triad', 'Single Color', 'Palette Blend', 'Color Cycle']
const THEMES = ['Custom', 'Bubblegum', 'Cyber Pop', 'Lagoon', 'Neon Soda', 'Jellyfish', 'Citrus Pop', 'Lavender Fizz']

interface Bubble {
    x: number
    y: number
    vx: number
    vy: number
    baseSize: number
    alpha: number
    mix: number
    phase: number
    driftBias: number
    paletteBand: number
    paletteBlend: number
}

interface RGB { r: number; g: number; b: number }
interface ThemePalette {
    primary: string
    secondary: string
    accent: string
}

const THEME_PALETTES: Record<string, ThemePalette> = {
    Custom:        { primary: '#ff4f9a', secondary: '#76fff1', accent: '#6f2dff' },
    Bubblegum:     { primary: '#ff4f9a', secondary: '#ff98c8', accent: '#ffd1eb' },
    'Cyber Pop':   { primary: '#08f7fe', secondary: '#ff06b5', accent: '#6f2dff' },
    Lagoon:        { primary: '#46f1dc', secondary: '#5da8ff', accent: '#1746ff' },
    'Neon Soda':   { primary: '#89ff53', secondary: '#18e4ff', accent: '#ff4ed1' },
    Jellyfish:     { primary: '#8a7cff', secondary: '#ff7fcf', accent: '#76fff1' },
    'Citrus Pop':  { primary: '#ffe15d', secondary: '#ff9a3d', accent: '#ff5478' },
    'Lavender Fizz': { primary: '#d0a4ff', secondary: '#ff88d4', accent: '#83c4ff' },
}

function clamp(value: number, min: number, max: number): number {
    if (Number.isNaN(value)) return min
    return Math.max(min, Math.min(max, value))
}

function rand(min: number, max: number): number {
    return Math.random() * (max - min) + min
}

function randomVelocity(): number {
    const value = rand(-1.0, 1.0)
    return Math.abs(value) < 0.08 ? (Math.random() < 0.5 ? -0.22 : 0.22) : value
}

function hexToRgb(hex: string): RGB {
    const normalized = hex.replace('#', '')
    const full = normalized.length === 3
        ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
        : normalized
    const parsed = parseInt(full, 16)
    if (Number.isNaN(parsed)) return { r: 255, g: 0, b: 102 }
    return { r: (parsed >> 16) & 255, g: (parsed >> 8) & 255, b: parsed & 255 }
}

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1)})`
}

function mixRgb(a: RGB, b: RGB, amount: number): RGB {
    const t = clamp(amount, 0, 1)
    return {
        r: Math.round(a.r + (b.r - a.r) * t),
        g: Math.round(a.g + (b.g - a.g) * t),
        b: Math.round(a.b + (b.b - a.b) * t),
    }
}

function hslToRgb(h: number, sPercent: number, lPercent: number): RGB {
    const s = clamp(sPercent, 0, 100) / 100
    const l = clamp(lPercent, 0, 100) / 100
    const c = (1 - Math.abs(2 * l - 1)) * s
    const hPrime = ((h % 360) + 360) % 360 / 60
    const x = c * (1 - Math.abs((hPrime % 2) - 1))

    let r = 0
    let g = 0
    let b = 0
    if (hPrime < 1) [r, g, b] = [c, x, 0]
    else if (hPrime < 2) [r, g, b] = [x, c, 0]
    else if (hPrime < 3) [r, g, b] = [0, c, x]
    else if (hPrime < 4) [r, g, b] = [0, x, c]
    else if (hPrime < 5) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    const m = l - c / 2
    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
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

function enrichRgb(color: RGB, saturationBoost: number, lightnessOffset = 0): RGB {
    const { h, s, l } = rgbToHsl(color)
    return hslToRgb(
        h,
        clamp((s + saturationBoost) * 100, 0, 100),
        clamp((l + lightnessOffset) * 100, 0, 100),
    )
}

function polishBubbleColors(colors: { aura: RGB; body: RGB; rim: RGB; gloss: RGB }) {
    return {
        aura: enrichRgb(colors.aura, 0.16, -0.02),
        body: enrichRgb(colors.body, 0.12, -0.06),
        rim: enrichRgb(colors.rim, 0.18, -0.01),
        gloss: enrichRgb(colors.gloss, 0.08, 0.01),
    }
}

function createBubbles(count: number, width: number, height: number): Bubble[] {
    const bubbles: Bubble[] = []
    for (let i = 0; i < count; i++) {
        const radius = rand(14, 28)
        bubbles.push({
            x: rand(radius, Math.max(radius + 1, width - radius)),
            y: rand(radius, Math.max(radius + 1, height - radius)),
            vx: randomVelocity(),
            vy: randomVelocity(),
            baseSize: radius,
            alpha: 0.34 + (i / Math.max(1, count - 1)) * 0.34,
            mix: Math.random(),
            phase: Math.random() * Math.PI * 2,
            driftBias: 0.84 + Math.random() * 0.44,
            paletteBand: Math.floor(Math.random() * 3),
            paletteBlend: Math.random(),
        })
    }
    return bubbles
}

function getPalette(theme: string, primary: string, secondary: string, accent: string): ThemePalette {
    if (theme === 'Custom') {
        return { primary, secondary, accent }
    }
    return THEME_PALETTES[theme] ?? THEME_PALETTES.Bubblegum
}

function pickPaletteColor(index: number, palette: ThemePalette): RGB {
    if (index === 0) return hexToRgb(palette.primary)
    if (index === 1) return hexToRgb(palette.secondary)
    return hexToRgb(palette.accent)
}

function pickPaletteSet(phase: number, palette: ThemePalette): { body: RGB; rim: RGB; gloss: RGB } {
    const band = Math.floor(fract(phase) * 3)
    if (band === 0) {
        return {
            body: pickPaletteColor(0, palette),
            rim: pickPaletteColor(1, palette),
            gloss: pickPaletteColor(2, palette),
        }
    }
    if (band === 1) {
        return {
            body: pickPaletteColor(1, palette),
            rim: pickPaletteColor(2, palette),
            gloss: pickPaletteColor(0, palette),
        }
    }
    return {
        body: pickPaletteColor(2, palette),
        rim: pickPaletteColor(0, palette),
        gloss: pickPaletteColor(1, palette),
    }
}

function fract(value: number): number {
    return value - Math.floor(value)
}

function paletteGradientColor(phase: number, palette: ThemePalette): RGB {
    const t = fract(phase)
    const primary = pickPaletteColor(0, palette)
    const secondary = pickPaletteColor(1, palette)
    const accent = pickPaletteColor(2, palette)

    if (t < 1 / 3) {
        return mixRgb(primary, secondary, smoothstep(0, 1 / 3, t))
    }
    if (t < 2 / 3) {
        return mixRgb(secondary, accent, smoothstep(1 / 3, 2 / 3, t))
    }
    return mixRgb(accent, primary, smoothstep(2 / 3, 1, t))
}

function resolveBubbleColors(
    bubble: Bubble,
    mode: string,
    theme: string,
    palette: ThemePalette,
    singleColor: string,
): { aura: RGB; body: RGB; rim: RGB; gloss: RGB } {
    if (mode === 'Palette Blend' || mode === 'Rainbow') {
        const body = paletteGradientColor(bubble.mix + bubble.paletteBlend * 0.28, palette)
        const rim = paletteGradientColor(bubble.mix + 0.26 + bubble.paletteBlend * 0.14, palette)
        const gloss = paletteGradientColor(bubble.mix + 0.54, palette)
        return polishBubbleColors({ aura: mixRgb(body, rim, 0.22), body, rim, gloss })
    }

    if (mode === 'Color Cycle') {
        const base = pickPaletteSet((bubble.paletteBand + 0.02) / 3, palette)
        const next = pickPaletteSet(((bubble.paletteBand + 1) % 3 + 0.02) / 3, palette)
        const blend = smoothstep(0.15, 0.85, bubble.paletteBlend)
        const body = mixRgb(base.body, next.body, blend * 0.52)
        const rim = mixRgb(base.rim, next.rim, blend * 0.4)
        const gloss = mixRgb(base.gloss, next.gloss, blend * 0.34)
        return polishBubbleColors({ aura: mixRgb(body, rim, 0.22), body, rim, gloss })
    }

    if (mode === 'Single Color') {
        const base = theme === 'Custom' ? hexToRgb(singleColor) : hexToRgb(palette.primary)
        const rim = hexToRgb(palette.secondary)
        const gloss = hexToRgb(palette.accent)
        return polishBubbleColors({ aura: mixRgb(base, rim, 0.24), body: base, rim, gloss })
    }

    let { body, rim, gloss } = pickPaletteSet((bubble.paletteBand + 0.02) / 3, palette)
    let aura = mixRgb(body, rim, 0.10)
    if (palette.primary === '#08f7fe') {
        if (bubble.paletteBand === 0) {
            body = hexToRgb(palette.secondary)
            rim = hexToRgb(palette.primary)
            gloss = hexToRgb(palette.accent)
        } else if (bubble.paletteBand === 1) {
            body = hexToRgb(palette.primary)
            rim = hexToRgb(palette.secondary)
            gloss = hexToRgb(palette.accent)
        } else {
            body = hexToRgb(palette.accent)
            rim = hexToRgb(palette.primary)
            gloss = hexToRgb(palette.secondary)
        }
        aura = mixRgb(hexToRgb(palette.primary), hexToRgb(palette.accent), 0.12)
    }
    return polishBubbleColors({ aura, body, rim, gloss })
}

function smoothstep(edge0: number, edge1: number, value: number): number {
    const t = clamp((value - edge0) / Math.max(0.0001, edge1 - edge0), 0, 1)
    return t * t * (3 - 2 * t)
}

function drawBackdrop(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    bgColor: string,
    palette: ThemePalette,
    time: number,
): void {
    ctx.fillStyle = bgColor
    ctx.fillRect(0, 0, width, height)

    const primary = hexToRgb(palette.primary)
    const secondary = hexToRgb(palette.secondary)
    const accent = hexToRgb(palette.accent)

    const wash = ctx.createRadialGradient(
        width * (0.24 + Math.sin(time * 0.18) * 0.06),
        height * (0.26 + Math.cos(time * 0.15) * 0.05),
        0,
        width * 0.34,
        height * 0.34,
        Math.max(width, height) * 0.86,
    )
    wash.addColorStop(0, rgba(primary, 0.14))
    wash.addColorStop(0.48, rgba(secondary, 0.08))
    wash.addColorStop(1, rgba(primary, 0.0))
    ctx.fillStyle = wash
    ctx.fillRect(0, 0, width, height)

    const veil = ctx.createLinearGradient(0, 0, width, height)
    veil.addColorStop(0, rgba(accent, 0.06))
    veil.addColorStop(1, rgba(secondary, 0.0))
    ctx.fillStyle = veil
    ctx.fillRect(0, 0, width, height)
}

export default canvas.stateful('Bubble Garden', {
    colorMode: COLOR_MODES,
    theme:     THEMES,
    bgColor:   '#05040a',
    color:     '#ff4f9a',
    color2:    '#ff98c8',
    color3:    '#76fff1',
    speed:     [0, 100, 10],
    size:      [1, 10, 5],
    count:     [10, 120, 36],
}, () => {
    let bubbles = createBubbles(36, 320, 200)
    let prevCount = 36
    let prevSpeed = 10

    return (ctx, time, c) => {
        const count = Math.max(10, Math.floor(c.count as number))
        const speed = c.speed as number
        const size = c.size as number
        const colorMode = c.colorMode as string
        const theme = c.theme as string
        const bgColor = c.bgColor as string
        const color = c.color as string
        const color2 = c.color2 as string
        const color3 = c.color3 as string
        const width = ctx.canvas.width
        const height = ctx.canvas.height

        if (count !== prevCount) {
            bubbles = createBubbles(count, width, height)
            prevCount = count
        } else if (speed !== prevSpeed) {
            for (const bubble of bubbles) {
                bubble.vx = randomVelocity()
                bubble.vy = randomVelocity()
            }
            prevSpeed = speed
        }

        const palette = getPalette(theme, color, color2, color3)
        drawBackdrop(ctx, width, height, bgColor, palette, time)

        const speedScale = Math.max(0, speed) / 12
        const sizeScale = Math.max(0.2, size / 5)

        for (let i = 0; i < bubbles.length; i++) {
            const bubble = bubbles[i]
            const pulse = 0.90 + 0.12 * Math.sin(time * (1.0 + bubble.driftBias * 0.4) + bubble.phase)
            const radius = bubble.baseSize * sizeScale * pulse

            if (speedScale > 0) {
                bubble.x += (bubble.vx * speedScale + Math.sin(time * 0.5 + bubble.phase) * 0.08) * bubble.driftBias
                bubble.y += (bubble.vy * speedScale + Math.cos(time * 0.42 + bubble.phase) * 0.06) * bubble.driftBias

                if (bubble.x + radius >= width) {
                    bubble.x = width - radius
                    bubble.vx = -Math.abs(bubble.vx)
                } else if (bubble.x - radius <= 0) {
                    bubble.x = radius
                    bubble.vx = Math.abs(bubble.vx)
                }

                if (bubble.y + radius >= height) {
                    bubble.y = height - radius
                    bubble.vy = -Math.abs(bubble.vy)
                } else if (bubble.y - radius <= 0) {
                    bubble.y = radius
                    bubble.vy = Math.abs(bubble.vy)
                }
            }

            const colors = resolveBubbleColors(bubble, colorMode, theme, palette, color)
            const bodyAlpha = bubble.alpha * 0.96
            const auraAlpha = bubble.alpha * 0.42
            const innerAlpha = bubble.alpha * 0.42

            ctx.fillStyle = rgba(colors.aura, auraAlpha)
            ctx.beginPath()
            ctx.arc(bubble.x, bubble.y, radius * 1.55, 0, Math.PI * 2)
            ctx.fill()

            ctx.fillStyle = rgba(colors.body, bodyAlpha)
            ctx.beginPath()
            ctx.arc(bubble.x, bubble.y, radius, 0, Math.PI * 2)
            ctx.fill()

            ctx.fillStyle = rgba(mixRgb(colors.body, colors.gloss, 0.34), innerAlpha)
            ctx.beginPath()
            ctx.arc(bubble.x - radius * 0.18, bubble.y - radius * 0.22, radius * 0.62, 0, Math.PI * 2)
            ctx.fill()

            ctx.strokeStyle = rgba(colors.rim, 0.48 + bubble.alpha * 0.22)
            ctx.lineWidth = Math.max(1, radius * 0.12)
            ctx.beginPath()
            ctx.arc(bubble.x, bubble.y, Math.max(1, radius - 0.5), 0, Math.PI * 2)
            ctx.stroke()

            ctx.fillStyle = rgba(colors.gloss, 0.26 + bubble.alpha * 0.20)
            ctx.beginPath()
            ctx.arc(bubble.x - radius * 0.30, bubble.y - radius * 0.32, Math.max(1, radius * 0.18), 0, Math.PI * 2)
            ctx.fill()
        }
    }
}, {
    description: 'Theme-rich bubble field with custom triads, colored rims, and glossy highlights',
})
