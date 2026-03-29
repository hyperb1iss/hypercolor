import { canvas, color, combo, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, num } from '@hypercolor/sdk'

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

interface RGB {
    r: number
    g: number
    b: number
}
interface ThemePalette {
    primary: string
    secondary: string
    accent: string
}

const THEME_PALETTES: Record<string, ThemePalette> = {
    Bubblegum: { accent: '#8a5cff', primary: '#ff4f9a', secondary: '#ff74c5' },
    'Citrus Pop': { accent: '#ff5478', primary: '#ffb347', secondary: '#ff7a2f' },
    Custom: { accent: '#6f2dff', primary: '#ff4f9a', secondary: '#80ffea' },
    'Cyber Pop': { accent: '#6f2dff', primary: '#08f7fe', secondary: '#ff06b5' },
    Jellyfish: { accent: '#76fff1', primary: '#8a7cff', secondary: '#ff7fcf' },
    Lagoon: { accent: '#1746ff', primary: '#46f1dc', secondary: '#5da8ff' },
    'Lavender Fizz': { accent: '#66d4ff', primary: '#9f72ff', secondary: '#ff5ec8' },
    'Neon Soda': { accent: '#ff4ed1', primary: '#36ff9a', secondary: '#18e4ff' },
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
    const full =
        normalized.length === 3
            ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
            : normalized
    const parsed = parseInt(full, 16)
    if (Number.isNaN(parsed)) return { b: 102, g: 0, r: 255 }
    return { b: parsed & 255, g: (parsed >> 8) & 255, r: (parsed >> 16) & 255 }
}

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1)})`
}

function mixRgb(a: RGB, b: RGB, amount: number): RGB {
    const t = clamp(amount, 0, 1)
    return {
        b: Math.round(a.b + (b.b - a.b) * t),
        g: Math.round(a.g + (b.g - a.g) * t),
        r: Math.round(a.r + (b.r - a.r) * t),
    }
}

function hslToRgb(h: number, sPercent: number, lPercent: number): RGB {
    const s = clamp(sPercent, 0, 100) / 100
    const l = clamp(lPercent, 0, 100) / 100
    const c = (1 - Math.abs(2 * l - 1)) * s
    const hPrime = (((h % 360) + 360) % 360) / 60
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
        b: Math.round((b + m) * 255),
        g: Math.round((g + m) * 255),
        r: Math.round((r + m) * 255),
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

    if (delta === 0) return { h: 0, l, s: 0 }

    const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let h = 0
    if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / delta + 2
    else h = (r - g) / delta + 4

    return { h: h * 60, l, s }
}

function enrichRgb(color: RGB, saturationBoost: number, lightnessOffset = 0): RGB {
    const { h, s, l } = rgbToHsl(color)
    return hslToRgb(h, clamp((s + saturationBoost) * 100, 0, 100), clamp((l + lightnessOffset) * 100, 0, 100))
}

function polishBubbleColors(colors: { aura: RGB; body: RGB; rim: RGB; gloss: RGB }) {
    return {
        aura: enrichRgb(colors.aura, 0.18, -0.08),
        body: enrichRgb(colors.body, 0.16, -0.12),
        gloss: enrichRgb(colors.gloss, 0.12, -0.02),
        rim: enrichRgb(colors.rim, 0.2, -0.06),
    }
}

function createBubbles(count: number, width: number, height: number): Bubble[] {
    const bubbles: Bubble[] = []
    for (let i = 0; i < count; i++) {
        const radius = rand(14, 28)
        bubbles.push({
            alpha: 0.22 + (i / Math.max(1, count - 1)) * 0.24,
            baseSize: radius,
            driftBias: 0.84 + Math.random() * 0.44,
            mix: Math.random(),
            paletteBand: Math.floor(Math.random() * 3),
            paletteBlend: Math.random(),
            phase: Math.random() * Math.PI * 2,
            vx: randomVelocity(),
            vy: randomVelocity(),
            x: rand(radius, Math.max(radius + 1, width - radius)),
            y: rand(radius, Math.max(radius + 1, height - radius)),
        })
    }
    return bubbles
}

function getPalette(theme: string, primary: string, secondary: string, accent: string): ThemePalette {
    if (theme === 'Custom') {
        return { accent, primary, secondary }
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
            gloss: pickPaletteColor(2, palette),
            rim: pickPaletteColor(1, palette),
        }
    }
    if (band === 1) {
        return {
            body: pickPaletteColor(1, palette),
            gloss: pickPaletteColor(0, palette),
            rim: pickPaletteColor(2, palette),
        }
    }
    return {
        body: pickPaletteColor(2, palette),
        gloss: pickPaletteColor(1, palette),
        rim: pickPaletteColor(0, palette),
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
        return polishBubbleColors({ aura: mixRgb(body, rim, 0.22), body, gloss, rim })
    }

    if (mode === 'Color Cycle') {
        const base = pickPaletteSet((bubble.paletteBand + 0.02) / 3, palette)
        const next = pickPaletteSet((((bubble.paletteBand + 1) % 3) + 0.02) / 3, palette)
        const blend = smoothstep(0.15, 0.85, bubble.paletteBlend)
        const body = mixRgb(base.body, next.body, blend * 0.52)
        const rim = mixRgb(base.rim, next.rim, blend * 0.4)
        const gloss = mixRgb(base.gloss, next.gloss, blend * 0.34)
        return polishBubbleColors({ aura: mixRgb(body, rim, 0.22), body, gloss, rim })
    }

    if (mode === 'Single Color') {
        const base = theme === 'Custom' ? hexToRgb(singleColor) : hexToRgb(palette.primary)
        const rim = hexToRgb(palette.secondary)
        const gloss = hexToRgb(palette.accent)
        return polishBubbleColors({ aura: mixRgb(base, rim, 0.24), body: base, gloss, rim })
    }

    let { body, rim, gloss } = pickPaletteSet((bubble.paletteBand + 0.02) / 3, palette)
    let aura = mixRgb(body, rim, 0.1)
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
    return polishBubbleColors({ aura, body, gloss, rim })
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

export default canvas.stateful(
    'Bubble Garden',
    {
        bgColor: color('Background', '#05040a', { group: 'Scene' }),
        color: color('Color', '#ff4f9a', { group: 'Color' }),
        color2: color('Color 2', '#80ffea', { group: 'Color' }),
        color3: color('Color 3', '#6f2dff', { group: 'Color' }),
        colorMode: combo('Color Mode', ['Color Cycle', 'Palette Blend', 'Single Color', 'Triad'], {
            default: 'Palette Blend',
            group: 'Color',
        }),
        count: num('Count', [10, 120], 30, { group: 'Scene' }),
        size: num('Size', [1, 10], 5, { group: 'Scene' }),
        speed: num('Speed', [0, 100], 10, { group: 'Motion' }),
        theme: combo(
            'Theme',
            ['Bubblegum', 'Citrus Pop', 'Custom', 'Cyber Pop', 'Jellyfish', 'Lagoon', 'Lavender Fizz', 'Neon Soda'],
            { default: 'Cyber Pop', group: 'Color' },
        ),
    },
    () => {
        let bubbles = createBubbles(30, DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT)
        let prevCount = 30
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
                const pulse = 0.9 + 0.12 * Math.sin(time * (1.0 + bubble.driftBias * 0.4) + bubble.phase)
                const radius = bubble.baseSize * sizeScale * pulse

                if (speedScale > 0) {
                    bubble.x += (bubble.vx * speedScale + Math.sin(time * 0.5 + bubble.phase) * 0.08) * bubble.driftBias
                    bubble.y +=
                        (bubble.vy * speedScale + Math.cos(time * 0.42 + bubble.phase) * 0.06) * bubble.driftBias

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
                const bodyAlpha = bubble.alpha * 0.82
                const auraAlpha = bubble.alpha * 0.22
                const innerAlpha = bubble.alpha * 0.28

                ctx.fillStyle = rgba(colors.aura, auraAlpha)
                ctx.beginPath()
                ctx.arc(bubble.x, bubble.y, radius * 1.2, 0, Math.PI * 2)
                ctx.fill()

                ctx.fillStyle = rgba(colors.body, bodyAlpha)
                ctx.beginPath()
                ctx.arc(bubble.x, bubble.y, radius, 0, Math.PI * 2)
                ctx.fill()

                ctx.fillStyle = rgba(mixRgb(colors.body, colors.gloss, 0.34), innerAlpha)
                ctx.beginPath()
                ctx.arc(bubble.x - radius * 0.18, bubble.y - radius * 0.22, radius * 0.62, 0, Math.PI * 2)
                ctx.fill()

                ctx.strokeStyle = rgba(colors.rim, 0.38 + bubble.alpha * 0.16)
                ctx.lineWidth = Math.max(1, radius * 0.12)
                ctx.beginPath()
                ctx.arc(bubble.x, bubble.y, Math.max(1, radius - 0.5), 0, Math.PI * 2)
                ctx.stroke()

                ctx.fillStyle = rgba(colors.gloss, 0.18 + bubble.alpha * 0.14)
                ctx.beginPath()
                ctx.arc(bubble.x - radius * 0.3, bubble.y - radius * 0.32, Math.max(1, radius * 0.18), 0, Math.PI * 2)
                ctx.fill()
            }
        }
    },
    {
        description:
            'Drift through a luminous bubble field — glossy spheres rise with colored rims catching light as they float, collide, and shimmer',
        presets: [
            {
                controls: {
                    bgColor: '#020108',
                    color: '#8a7cff',
                    color2: '#ff7fcf',
                    color3: '#76fff1',
                    colorMode: 'Palette Blend',
                    count: 65,
                    size: 7,
                    speed: 8,
                    theme: 'Jellyfish',
                },
                description:
                    'Colonial organisms drift in eternal darkness — each translucent bell a separate creature chained in bioluminescent congress',
                name: 'Bathypelagic Siphonophore',
            },
            {
                controls: {
                    bgColor: '#0e0800',
                    color: '#ffcc33',
                    color2: '#ff7a2f',
                    color3: '#ff5478',
                    colorMode: 'Single Color',
                    count: 120,
                    size: 3,
                    speed: 45,
                    theme: 'Citrus Pop',
                },
                description:
                    'Golden effervescence erupts from the bottle — a billion tiny spheres racing upward through amber light',
                name: 'Champagne Supernova',
            },
            {
                controls: {
                    bgColor: '#040a02',
                    color: '#36ff9a',
                    color2: '#18e4ff',
                    color3: '#ff4ed1',
                    colorMode: 'Triad',
                    count: 42,
                    size: 6,
                    speed: 18,
                    theme: 'Neon Soda',
                },
                description:
                    'Chemical bubbles surface through contaminated sediment — each one a pressurized capsule of fluorescent mutation',
                name: 'Toxic Waste Lagoon',
            },
            {
                controls: {
                    bgColor: '#08060e',
                    color: '#9f72ff',
                    color2: '#ff5ec8',
                    color3: '#66d4ff',
                    colorMode: 'Color Cycle',
                    count: 22,
                    size: 9,
                    speed: 5,
                    theme: 'Lavender Fizz',
                },
                description:
                    'Razor-thin membranes refract white light into impossible rainbows — each bubble a floating physics experiment',
                name: 'Soap Film Interference',
            },
            {
                controls: {
                    bgColor: '#0a0208',
                    color: '#ff4f9a',
                    color2: '#ff74c5',
                    color3: '#8a5cff',
                    colorMode: 'Palette Blend',
                    count: 85,
                    size: 4,
                    speed: 12,
                    theme: 'Bubblegum',
                },
                description:
                    'Endosomes shuttle through cellular fluid — lipid bilayer spheres ferrying molecular cargo in warm biological pink',
                name: 'Cytoplasmic Vesicle Transport',
            },
            {
                controls: {
                    bgColor: '#000810',
                    color: '#46f1dc',
                    color2: '#5da8ff',
                    color3: '#1746ff',
                    colorMode: 'Triad',
                    count: 18,
                    size: 10,
                    speed: 3,
                    theme: 'Lagoon',
                },
                description:
                    'Ancient glass fishing floats drift in a midnight cove — massive teal orbs bob on black water, each one holding a trapped sunrise',
                name: 'Moonlit Glass Floats',
            },
            {
                controls: {
                    bgColor: '#02050a',
                    color: '#08f7fe',
                    color2: '#ff06b5',
                    color3: '#6f2dff',
                    colorMode: 'Color Cycle',
                    count: 100,
                    size: 2,
                    speed: 65,
                    theme: 'Cyber Pop',
                },
                description:
                    'Particle accelerator collision event — a hundred luminous fragments scatter from the impact point in cyan, magenta, and ultraviolet',
                name: 'Hadron Splash',
            },
        ],
    },
)
