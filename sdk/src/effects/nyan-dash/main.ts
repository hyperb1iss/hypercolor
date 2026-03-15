import { canvas, combo, normalizeSpeed, num } from '@hypercolor/sdk'

type MotionMode = 'Original' | 'Dash' | 'Hyper'
type RainbowMode = 'Original' | 'Wavy' | 'Party'
type CatTheme = 'Classic' | 'Blueberry' | 'Mint' | 'Midnight'

interface Star {
    drift: number
    phase: number
    size: number
    speed: number
    twinkle: number
    x: number
    y: number
}

interface CatPalette {
    blush: string
    fur: string
    furDark: string
    frosting: string
    nose: string
    outline: string
    pastry: string
    pupil: string
    sprinkle: string
    white: string
}

interface CatFrame {
    bodyDy: number
    headDx: number
    headDy: number
    pawFrame: number
    tailFrame: number
}

type RectSpec = readonly [number, number, number, number]

const MOTION_MODES = ['Original', 'Dash', 'Hyper'] as const
const RAINBOW_MODES = ['Original', 'Wavy', 'Party'] as const
const CAT_THEMES = ['Classic', 'Blueberry', 'Mint', 'Midnight'] as const

const CLASSIC_RAINBOW = ['#ff2d2d', '#ff9d17', '#ffe94d', '#36ff2a', '#1ea7ff', '#6d50ff'] as const

const CAT_THEMES_PALETTE: Record<CatTheme, CatPalette> = {
    Blueberry: {
        blush: '#ffb0d8',
        frosting: '#8ed8ff',
        fur: '#a2a7c7',
        furDark: '#70759b',
        nose: '#ff7ac8',
        outline: '#000000',
        pastry: '#dfc6a7',
        pupil: '#2f3554',
        sprinkle: '#f7f2ff',
        white: '#ffffff',
    },
    Classic: {
        blush: '#ff9fb8',
        frosting: '#ff99cc',
        fur: '#999999',
        furDark: '#686868',
        nose: '#ff3399',
        outline: '#000000',
        pastry: '#ffcc99',
        pupil: '#333333',
        sprinkle: '#ff4fa0',
        white: '#ffffff',
    },
    Midnight: {
        blush: '#ff7db0',
        frosting: '#9a78ff',
        fur: '#70707f',
        furDark: '#49495a',
        nose: '#ff6ac1',
        outline: '#000000',
        pastry: '#bca2d8',
        pupil: '#0f1020',
        sprinkle: '#ffe073',
        white: '#ffffff',
    },
    Mint: {
        blush: '#ffb1c7',
        frosting: '#92ffd7',
        fur: '#a9ada4',
        furDark: '#73786d',
        nose: '#ff6eb5',
        outline: '#000000',
        pastry: '#f3d7a8',
        pupil: '#2d352e',
        sprinkle: '#42dca3',
        white: '#ffffff',
    },
}

const CAT_FRAMES: readonly CatFrame[] = [
    { bodyDy: 1, headDx: 0, headDy: 0, pawFrame: 0, tailFrame: 0 },
    { bodyDy: 1, headDx: 1, headDy: 0, pawFrame: 1, tailFrame: 1 },
    { bodyDy: 0, headDx: 1, headDy: 0, pawFrame: 2, tailFrame: 2 },
    { bodyDy: 0, headDx: 1, headDy: 0, pawFrame: 1, tailFrame: 3 },
    { bodyDy: 0, headDx: 0, headDy: 0, pawFrame: 3, tailFrame: 4 },
    { bodyDy: 0, headDx: 0, headDy: 1, pawFrame: 4, tailFrame: 1 },
]

const BODY_SPRINKLES: readonly RectSpec[] = [
    [64, 18, 6, 6],
    [92, 12, 6, 6],
    [110, 12, 6, 6],
    [87, 35, 6, 6],
    [132, 24, 6, 6],
    [70, 46, 6, 6],
    [92, 52, 6, 6],
    [58, 58, 6, 6],
    [81, 69, 6, 6],
]

const BODY_PASTRY_TRIM: readonly RectSpec[] = [
    [52, 6, 12, 6],
    [137, 6, 12, 6],
    [52, 80, 12, 6],
    [137, 80, 12, 6],
    [52, 6, 6, 12],
    [143, 6, 6, 12],
    [52, 74, 6, 12],
    [143, 74, 6, 12],
]

const EAR_OUTLINE_RECTS: readonly RectSpec[] = [
    [0, 6, 6, 24],
    [6, 0, 12, 6],
    [30, 18, 12, 6],
    [18, 6, 6, 6],
    [24, 12, 6, 6],
]

const EAR_FILL_RECTS: readonly RectSpec[] = [
    [6, 6, 12, 30],
    [18, 18, 12, 12],
    [30, 24, 12, 6],
    [18, 12, 6, 6],
]

const FACE_OUTLINE_RECTS: readonly RectSpec[] = [
    [23, 6, 12, 12],
    [62, 6, 12, 12],
    [48, 12, 6, 6],
    [29, 24, 6, 6],
    [46, 24, 6, 6],
    [62, 24, 6, 6],
]

const FACE_HIGHLIGHTS: readonly RectSpec[] = [
    [23, 6, 6, 6],
    [62, 6, 6, 6],
]

const FACE_BLUSH_RECTS: readonly RectSpec[] = [
    [11, 18, 12, 12],
    [74, 18, 12, 12],
]

const CHIN_OUTLINE_RECTS: readonly RectSpec[] = [
    [6, 30, 6, 6],
    [12, 36, 6, 6],
    [80, 30, 6, 6],
    [74, 36, 6, 6],
    [18, 42, 56, 6],
    [29, 30, 39, 6],
]

const CHIN_FILL_RECTS: readonly RectSpec[] = [
    [12, 30, 68, 6],
    [18, 36, 56, 6],
]

const TAIL_OUTLINES: readonly (readonly RectSpec[])[] = [
    [
        [6, 0, 23, 18],
        [11, 6, 23, 18],
        [17, 11, 23, 18],
        [23, 17, 23, 18],
        [34, 23, 6, 18],
    ],
    [
        [12, 6, 11, 23],
        [18, 12, 11, 23],
        [29, 17, 11, 23],
        [6, 12, 6, 11],
    ],
    [
        [16, 24, 24, 12],
        [4, 30, 24, 12],
        [10, 36, 24, 12],
        [34, 18, 6, 24],
    ],
    [
        [28, 18, 12, 24],
        [16, 24, 12, 24],
        [10, 30, 12, 24],
        [4, 36, 6, 12],
    ],
    [
        [6, 6, 24, 18],
        [12, 12, 24, 18],
        [0, 12, 6, 12],
        [36, 12, 6, 12],
        [28, 30, 12, 6],
    ],
]

const TAIL_FILLS: readonly (readonly RectSpec[])[] = [
    [
        [11, 6, 11, 6],
        [17, 12, 11, 6],
        [23, 18, 11, 6],
        [29, 24, 11, 6],
        [35, 30, 5, 6],
    ],
    [
        [12, 12, 11, 6],
        [12, 17, 11, 6],
        [18, 23, 11, 6],
        [29, 23, 11, 6],
        [29, 29, 11, 6],
    ],
    [
        [16, 30, 24, 6],
        [10, 36, 18, 6],
    ],
    [
        [28, 24, 12, 12],
        [10, 36, 12, 12],
        [16, 30, 12, 6],
    ],
    [
        [6, 12, 18, 6],
        [12, 18, 22, 6],
        [34, 24, 6, 6],
    ],
]

const PAW_OUTLINES: readonly (readonly RectSpec[])[] = [
    [
        [28, 98, 18, 18],
        [34, 92, 18, 18],
        [58, 92, 18, 18],
        [64, 98, 18, 18],
        [118, 92, 18, 18],
        [125, 98, 18, 18],
        [149, 92, 18, 18],
        [155, 98, 18, 18],
    ],
    [
        [34, 98, 18, 18],
        [40, 92, 18, 18],
        [64, 92, 18, 18],
        [70, 98, 18, 18],
        [119, 92, 18, 18],
        [125, 98, 18, 18],
        [149, 92, 18, 18],
        [155, 98, 18, 18],
    ],
    [
        [40, 98, 18, 18],
        [46, 92, 18, 18],
        [70, 92, 18, 18],
        [76, 98, 18, 18],
        [124, 92, 18, 18],
        [130, 98, 18, 18],
        [155, 92, 18, 18],
        [161, 98, 18, 18],
    ],
    [
        [28, 98, 18, 18],
        [34, 92, 18, 18],
        [64, 92, 18, 18],
        [58, 98, 18, 18],
        [125, 92, 18, 18],
        [119, 98, 18, 18],
        [149, 92, 18, 18],
        [155, 98, 18, 18],
    ],
    [
        [28, 98, 18, 18],
        [34, 92, 18, 18],
        [58, 92, 18, 18],
        [64, 98, 18, 18],
        [118, 92, 18, 18],
        [125, 98, 18, 18],
        [149, 92, 18, 18],
        [155, 98, 18, 18],
    ],
]

const PAW_FILLS: readonly (readonly RectSpec[])[] = [
    [
        [34, 98, 12, 12],
        [64, 98, 12, 12],
        [125, 98, 12, 12],
        [155, 98, 12, 12],
        [46, 98, 6, 6],
    ],
    [
        [40, 98, 12, 12],
        [70, 98, 12, 12],
        [125, 98, 12, 12],
        [155, 98, 12, 12],
    ],
    [
        [46, 98, 12, 12],
        [76, 98, 12, 12],
        [130, 98, 12, 12],
        [161, 98, 12, 12],
    ],
    [
        [34, 98, 12, 12],
        [40, 92, 12, 12],
        [64, 98, 12, 12],
        [125, 98, 12, 12],
        [155, 98, 12, 12],
    ],
    [
        [34, 98, 12, 12],
        [40, 92, 12, 12],
        [64, 98, 12, 12],
        [125, 98, 12, 12],
        [155, 98, 12, 12],
    ],
]

const STAR_OFFSETS: readonly [number, number][] = [
    [20, 28],
    [170, 68],
    [320, 118],
    [200, 168],
]

const CSS_SPARK_BASES: readonly [number, number, number][] = [
    [20, 0, 0],
    [170, 40, 0.2],
    [320, 100, 0.4],
    [200, 150, 0.6],
]

const CSS_SPARK_PHASES: readonly (readonly RectSpec[])[] = [
    [[17, 17, 6, 6]],
    [
        [17, 0, 6, 6],
        [34, 17, 6, 6],
        [17, 34, 6, 6],
        [0, 17, 6, 6],
    ],
    [
        [17, 0, 6, 6],
        [34, 17, 6, 6],
        [17, 34, 6, 6],
        [0, 17, 6, 6],
        [6, 6, 5, 5],
        [29, 6, 5, 5],
        [29, 29, 5, 5],
        [6, 29, 5, 5],
    ],
    [
        [17, 0, 6, 11],
        [17, 29, 6, 11],
        [0, 17, 11, 6],
        [29, 17, 11, 6],
    ],
    [
        [17, 6, 6, 11],
        [17, 23, 6, 11],
        [6, 17, 6, 6],
        [23, 17, 6, 6],
    ],
    [
        [17, 12, 5, 5],
        [17, 22, 5, 5],
        [11, 17, 5, 5],
        [22, 17, 5, 5],
    ],
]

const CAT_FRAME_RATE = 1 / 0.07
const RAINBOW_STEP_RATE = 1 / 0.35
const CSS_CAT_WIDTH = 194
const CSS_CAT_HEIGHT = 122
const SPRITE_SCALE_BASE = 0.55

function clamp(value: number, min: number, max: number): number {
    if (Number.isNaN(value)) return min
    return Math.max(min, Math.min(max, value))
}

function hash(value: number): number {
    const hashed = Math.sin(value * 127.1 + 311.7) * 43758.5453123
    return hashed - Math.floor(hashed)
}

function hsl(hue: number, saturation: number, lightness: number): string {
    const wrappedHue = ((hue % 360) + 360) % 360
    return `hsl(${wrappedHue}, ${clamp(saturation, 0, 100)}%, ${clamp(lightness, 0, 100)}%)`
}

function buildStars(count: number): Star[] {
    const stars: Star[] = []

    for (let index = 0; index < count; index++) {
        stars.push({
            drift: hash(index * 6.17 + 2.3) * Math.PI * 2,
            phase: hash(index * 3.19 + 1.2) * Math.PI * 2,
            size: 1 + Math.floor(hash(index * 4.01 + 7.7) * 2),
            speed: 24 + hash(index * 5.27 + 1.4) * 34,
            twinkle: 0.8 + hash(index * 2.91 + 4.8) * 2.4,
            x: hash(index * 1.87 + 9.1),
            y: hash(index * 2.47 + 5.4),
        })
    }

    return stars
}

function pickMotionMode(value: string): MotionMode {
    return MOTION_MODES.includes(value as MotionMode) ? (value as MotionMode) : 'Original'
}

function pickRainbowMode(value: string): RainbowMode {
    return RAINBOW_MODES.includes(value as RainbowMode) ? (value as RainbowMode) : 'Original'
}

function pickCatTheme(value: string): CatTheme {
    return CAT_THEMES.includes(value as CatTheme) ? (value as CatTheme) : 'Classic'
}

function rainbowColor(index: number, mode: RainbowMode, time: number): string {
    if (mode === 'Party') {
        return hsl(time * 55 + index * 44, 95, 58)
    }

    return CLASSIC_RAINBOW[index] ?? CLASSIC_RAINBOW[0]
}

function drawScaledRect(
    ctx: CanvasRenderingContext2D,
    originX: number,
    originY: number,
    scale: number,
    [x, y, width, height]: RectSpec,
    color: string,
): void {
    ctx.fillStyle = color
    ctx.fillRect(
        Math.round(originX + x * scale),
        Math.round(originY + y * scale),
        Math.max(1, Math.round(width * scale)),
        Math.max(1, Math.round(height * scale)),
    )
}

function drawScaledRects(
    ctx: CanvasRenderingContext2D,
    originX: number,
    originY: number,
    scale: number,
    rects: readonly RectSpec[],
    color: string,
): void {
    for (const rect of rects) {
        drawScaledRect(ctx, originX, originY, scale, rect, color)
    }
}

function drawMirroredRects(
    ctx: CanvasRenderingContext2D,
    originX: number,
    originY: number,
    scale: number,
    mirrorWidth: number,
    rects: readonly RectSpec[],
    color: string,
): void {
    for (const [x, y, width, height] of rects) {
        drawScaledRect(ctx, originX, originY, scale, [mirrorWidth - x - width, y, width, height], color)
    }
}

function drawExactBody(
    ctx: CanvasRenderingContext2D,
    originX: number,
    originY: number,
    scale: number,
    palette: CatPalette,
): void {
    drawScaledRect(ctx, originX, originY, scale, [46, 0, 109, 92], palette.pastry)
    drawScaledRect(ctx, originX, originY, scale, [40, 6, 6, 80], palette.outline)
    drawScaledRect(ctx, originX, originY, scale, [149, 6, 6, 80], palette.outline)
    drawScaledRect(ctx, originX, originY, scale, [52, 0, 97, 6], palette.outline)
    drawScaledRect(ctx, originX, originY, scale, [52, 86, 97, 6], palette.outline)
    drawScaledRects(
        ctx,
        originX,
        originY,
        scale,
        [
            [46, 0, 6, 6],
            [149, 0, 6, 6],
            [46, 86, 6, 6],
            [149, 86, 6, 6],
        ],
        palette.outline,
    )

    drawScaledRect(ctx, originX, originY, scale, [52, 6, 97, 80], palette.frosting)
    drawScaledRects(ctx, originX, originY, scale, BODY_PASTRY_TRIM, palette.pastry)
    drawScaledRects(ctx, originX, originY, scale, BODY_SPRINKLES, palette.sprinkle)
}

function drawExactHead(
    ctx: CanvasRenderingContext2D,
    originX: number,
    originY: number,
    scale: number,
    frame: CatFrame,
    palette: CatPalette,
): void {
    const headX = originX + (102 + frame.headDx * 6) * scale
    const headY = originY + (56 + frame.headDy * 6) * scale

    drawScaledRects(ctx, headX, headY - 30 * scale, scale, EAR_OUTLINE_RECTS, palette.outline)
    drawScaledRects(ctx, headX, headY - 30 * scale, scale, EAR_FILL_RECTS, palette.fur)
    drawMirroredRects(ctx, headX + 52 * scale, headY - 30 * scale, scale, 42, EAR_OUTLINE_RECTS, palette.outline)
    drawMirroredRects(ctx, headX + 52 * scale, headY - 30 * scale, scale, 42, EAR_FILL_RECTS, palette.fur)

    drawScaledRect(ctx, headX, headY, scale, [0, 0, 6, 30], palette.outline)
    drawScaledRect(ctx, headX, headY, scale, [6, 0, 80, 30], palette.fur)
    drawScaledRect(ctx, headX, headY, scale, [86, 0, 6, 30], palette.outline)

    drawScaledRects(ctx, headX, headY, scale, FACE_OUTLINE_RECTS, palette.outline)
    drawScaledRects(ctx, headX, headY, scale, FACE_HIGHLIGHTS, palette.white)
    drawScaledRects(ctx, headX, headY, scale, FACE_BLUSH_RECTS, palette.blush)
    drawScaledRects(ctx, headX, headY, scale, CHIN_OUTLINE_RECTS, palette.outline)
    drawScaledRects(ctx, headX, headY, scale, CHIN_FILL_RECTS, palette.fur)
}

function drawExactTail(
    ctx: CanvasRenderingContext2D,
    originX: number,
    originY: number,
    scale: number,
    frameIndex: number,
    palette: CatPalette,
): void {
    const outlineRects = TAIL_OUTLINES[frameIndex] ?? TAIL_OUTLINES[0]
    const fillRects = TAIL_FILLS[frameIndex] ?? TAIL_FILLS[0]
    drawScaledRects(ctx, originX, originY, scale, outlineRects, palette.outline)
    drawScaledRects(ctx, originX, originY, scale, fillRects, palette.fur)
}

function drawExactPaws(
    ctx: CanvasRenderingContext2D,
    originX: number,
    originY: number,
    scale: number,
    frameIndex: number,
    palette: CatPalette,
): void {
    const outlineRects = PAW_OUTLINES[frameIndex] ?? PAW_OUTLINES[0]
    const fillRects = PAW_FILLS[frameIndex] ?? PAW_FILLS[0]
    drawScaledRects(ctx, originX, originY, scale, outlineRects, palette.outline)
    drawScaledRects(ctx, originX, originY, scale, fillRects, palette.fur)
}

function drawCat(
    ctx: CanvasRenderingContext2D,
    centerX: number,
    centerY: number,
    scale: number,
    frameIndex: number,
    palette: CatPalette,
): void {
    const frame = CAT_FRAMES[frameIndex] ?? CAT_FRAMES[0]
    const originX = centerX - (CSS_CAT_WIDTH * scale) / 2
    const originY = centerY - (CSS_CAT_HEIGHT * scale) / 2 + frame.bodyDy * 6 * scale

    drawExactTail(ctx, originX, originY + 40 * scale, scale, frame.tailFrame, palette)
    drawExactPaws(ctx, originX, originY, scale, frame.pawFrame, palette)
    drawExactBody(ctx, originX, originY, scale, palette)
    drawExactHead(ctx, originX, originY, scale, frame, palette)
}

function drawBackground(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    motionMode: MotionMode,
    time: number,
): void {
    if (motionMode === 'Hyper') {
        const gradient = ctx.createLinearGradient(0, 0, 0, height)
        gradient.addColorStop(0, '#072859')
        gradient.addColorStop(1, '#001a3d')
        ctx.fillStyle = gradient
    } else {
        ctx.fillStyle = '#003366'
    }

    ctx.fillRect(0, 0, width, height)

    if (motionMode === 'Hyper') {
        ctx.fillStyle = 'rgba(255, 255, 255, 0.04)'
        for (let line = 0; line < height; line += 12) {
            ctx.fillRect(0, line, width, 2)
        }

        ctx.fillStyle = 'rgba(255, 255, 255, 0.05)'
        for (let x = -40; x < width + 40; x += 36) {
            const skew = Math.sin(time * 2.4 + x * 0.03) * 8
            ctx.fillRect(x, height * 0.2 + skew, 12, height * 0.6)
        }
    }
}

function drawTwinkle(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    unit: number,
    frame: number,
    size: number,
    color: string,
): void {
    ctx.fillStyle = color

    if (frame === 0) {
        ctx.fillRect(
            Math.round(x),
            Math.round(y),
            Math.max(1, Math.round(unit * size)),
            Math.max(1, Math.round(unit * size)),
        )
        return
    }

    const arm = Math.max(1, Math.round(unit * size))
    const center = Math.max(1, Math.round(unit * size))

    ctx.fillRect(Math.round(x), Math.round(y + arm), center * 3, center)
    ctx.fillRect(Math.round(x + arm), Math.round(y), center, center * 3)

    if (frame >= 2) {
        ctx.fillRect(Math.round(x + arm), Math.round(y + arm), center, center)
    }

    if (frame >= 3) {
        ctx.fillRect(Math.round(x), Math.round(y), center, center)
        ctx.fillRect(Math.round(x + arm * 2), Math.round(y), center, center)
        ctx.fillRect(Math.round(x), Math.round(y + arm * 2), center, center)
        ctx.fillRect(Math.round(x + arm * 2), Math.round(y + arm * 2), center, center)
    }
}

function drawStars(
    ctx: CanvasRenderingContext2D,
    stars: Star[],
    width: number,
    height: number,
    time: number,
    speed: number,
    motionMode: MotionMode,
    rainbowMode: RainbowMode,
    starDensity: number,
    cssScale: number,
): void {
    if (motionMode !== 'Hyper') {
        const cycle = ((time * speed) / 0.7) % 1
        const scroll = ((time * speed * 400) / 0.7) * cssScale
        const loopWidth = 400 * cssScale
        const sparkWidth = 40 * cssScale
        const travelWidth = width + loopWidth + sparkWidth
        const rowHeight = 300 * cssScale
        const opacity = clamp(starDensity / 60, 0, 1)

        ctx.save()
        ctx.globalAlpha = opacity

        for (let row = -rowHeight; row < height + rowHeight; row += rowHeight) {
            for (const [baseX, baseY, phaseOffset] of CSS_SPARK_BASES) {
                const phase = (cycle + phaseOffset) % 1
                const phaseIndex =
                    phase < 0.16 ? 0 : phase < 0.33 ? 1 : phase < 0.5 ? 2 : phase < 0.66 ? 3 : phase < 0.83 ? 4 : 5
                const sparkX =
                    ((((width + baseX * cssScale - scroll) % travelWidth) + travelWidth) % travelWidth) - sparkWidth
                const sparkY = row + baseY * cssScale
                const color = rainbowMode === 'Party' ? hsl(time * 80 + baseX * 0.2, 92, 84) : '#ffffff'
                drawScaledRects(
                    ctx,
                    sparkX,
                    sparkY,
                    cssScale,
                    CSS_SPARK_PHASES[phaseIndex] ?? CSS_SPARK_PHASES[0],
                    color,
                )
            }
        }

        ctx.restore()
        return
    }

    const worldSpeed = motionMode === 'Hyper' ? 2.4 : 1.0
    const loopWidth = width + 80

    for (const [index, seed] of stars.entries()) {
        const scroll = time * seed.speed * speed * worldSpeed
        const x = ((((seed.x * loopWidth - scroll) % loopWidth) + loopWidth) % loopWidth) - 20
        const y = 12 + seed.y * (height - 24) + Math.sin(time * 0.9 + seed.drift) * 3
        const frame = Math.floor(time * seed.twinkle * 2 + seed.phase) % 4
        const color = rainbowMode === 'Party' ? hsl(time * 80 + index * 23, 92, 84) : '#ffffff'

        drawTwinkle(ctx, x, y, seed.size, frame, seed.size, color)
    }

    if (motionMode === 'Hyper') {
        ctx.strokeStyle = 'rgba(255, 255, 255, 0.2)'
        ctx.lineWidth = 1.5

        for (const [index, base] of STAR_OFFSETS.entries()) {
            const offset = ((time * 220 * speed + base[0]) % (width + 120)) - 60
            const y = 28 + base[1] * (height / 220)
            ctx.beginPath()
            ctx.moveTo(width - offset, y)
            ctx.lineTo(width - offset - 42 - index * 8, y)
            ctx.stroke()
        }
    }
}

function drawRainbow(
    ctx: CanvasRenderingContext2D,
    trailRight: number,
    centerY: number,
    cssScale: number,
    time: number,
    speed: number,
    rainbowMode: RainbowMode,
): void {
    if (trailRight <= 0) return

    const bandHeight = 17 * cssScale
    const tileWidth = 95 * cssScale
    const fillWidth = 49 * cssScale
    const secondWaveOffset = 46 * cssScale
    const frameStep = Math.floor(time * RAINBOW_STEP_RATE * speed) % 2
    const waveATop = centerY + (frameStep === 0 ? -54 : -60) * cssScale
    const waveBTop = centerY + (frameStep === 0 ? -60 : -54) * cssScale
    const rippleStrength = rainbowMode === 'Wavy' || rainbowMode === 'Party' ? 3 * cssScale : 0

    for (let bandIndex = 0; bandIndex < CLASSIC_RAINBOW.length; bandIndex++) {
        const color = rainbowColor(bandIndex, rainbowMode, time)
        const yA = waveATop + bandIndex * bandHeight
        const yB = waveBTop + bandIndex * bandHeight

        ctx.fillStyle = color

        for (let x = -tileWidth; x < trailRight + tileWidth; x += tileWidth) {
            const waveA = rippleStrength === 0 ? 0 : Math.sin(time * 4 + bandIndex * 0.4) * rippleStrength
            const waveB = rippleStrength === 0 ? 0 : Math.sin(time * 4 + bandIndex * 0.4 + Math.PI / 2) * rippleStrength

            if (x < trailRight) {
                const widthA = Math.min(fillWidth, trailRight - x)
                if (widthA > 0) {
                    ctx.fillRect(Math.round(x), Math.round(yA + waveA), Math.round(widthA), Math.round(bandHeight))
                }
            }

            const secondX = x + secondWaveOffset
            if (secondX < trailRight) {
                const widthB = Math.min(fillWidth, trailRight - secondX)
                if (widthB > 0) {
                    ctx.fillRect(
                        Math.round(secondX),
                        Math.round(yB + waveB),
                        Math.round(widthB),
                        Math.round(bandHeight),
                    )
                }
            }
        }
    }
}

canvas(
    'Nyan Dash',
    {
        motionMode: combo('Motion', [...MOTION_MODES], { group: 'Motion' }),
        rainbowMode: combo('Rainbow', [...RAINBOW_MODES], { group: 'Style' }),
        catTheme: combo('Cat Theme', [...CAT_THEMES], { group: 'Style' }),
        animationSpeed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        scale: num('Scale', [50, 180], 100, { group: 'Layout' }),
        positionX: num('Position X', [-100, 100], 0, { group: 'Layout' }),
        positionY: num('Position Y', [-100, 100], 0, { group: 'Layout' }),
        starDensity: num('Star Density', [0, 100], 50, { group: 'Atmosphere' }),
    },
    () => {
        let stars: Star[] = []
        let starCount = -1

        return (ctx, time, controls) => {
            const width = ctx.canvas.width
            const height = ctx.canvas.height
            const speed = normalizeSpeed(controls.animationSpeed as number)
            const scaleControl = clamp(controls.scale as number, 50, 180)
            const positionX = clamp(controls.positionX as number, -100, 100)
            const positionY = clamp(controls.positionY as number, -100, 100)
            const starDensity = clamp(controls.starDensity as number, 0, 100)
            const motionMode = pickMotionMode(String(controls.motionMode))
            const rainbowMode = pickRainbowMode(String(controls.rainbowMode))
            const catTheme = pickCatTheme(String(controls.catTheme))

            const nextStarCount = Math.floor(starDensity * 1.4)
            if (nextStarCount !== starCount) {
                starCount = nextStarCount
                stars = buildStars(starCount)
            }

            const cssScale = SPRITE_SCALE_BASE * (scaleControl / 100)
            const unit = Math.max(1, Math.round(6 * cssScale))
            const catWidth = CSS_CAT_WIDTH * cssScale
            const catHeight = CSS_CAT_HEIGHT * cssScale
            const frameIndex = Math.floor(time * CAT_FRAME_RATE * speed) % CAT_FRAMES.length

            drawBackground(ctx, width, height, motionMode, time)
            drawStars(ctx, stars, width, height, time, speed, motionMode, rainbowMode, starDensity, cssScale)

            const offsetX = (positionX / 100) * width * 0.36
            const offsetY = (positionY / 100) * height * 0.34

            let centerX = width * 0.5 + offsetX
            if (motionMode === 'Dash') {
                const travel = ((time * speed * 42) % (width + catWidth * 1.6)) - catWidth * 0.8
                centerX = width - travel + offsetX
            } else if (motionMode === 'Hyper') {
                centerX += Math.sin(time * speed * 1.5) * unit * 1.2
            }

            const centerY = clamp(height * 0.52 + offsetY, catHeight * 0.25, height - catHeight * 0.2)
            const trailEnd = centerX - catWidth / 2 + 52 * cssScale

            drawRainbow(ctx, trailEnd, centerY, cssScale, time, speed, rainbowMode)
            drawCat(ctx, centerX, centerY, cssScale, frameIndex, CAT_THEMES_PALETTE[catTheme])
        }
    },
    {
        description:
            'A faithful Nyan Cat pass driven by the classic CSS timing: stepped rainbow bands, chunky pixel stars, original-space bobbing, plus a few remix controls that stay out of the default look.',
        presets: [
            {
                name: 'Original Loop',
                description:
                    'Stationary cat, classic colors, and the stepped rainbow cadence that feels closest to the original loop.',
                controls: {
                    animationSpeed: 5,
                    catTheme: 'Classic',
                    motionMode: 'Original',
                    positionX: 0,
                    positionY: 0,
                    rainbowMode: 'Original',
                    scale: 100,
                    starDensity: 52,
                },
            },
            {
                name: 'CSS Cat Sprint',
                description:
                    'The cat actually dashes across the canvas while the old-school rainbow bands keep marching behind it.',
                controls: {
                    animationSpeed: 7,
                    catTheme: 'Classic',
                    motionMode: 'Dash',
                    positionX: 0,
                    positionY: -8,
                    rainbowMode: 'Original',
                    scale: 92,
                    starDensity: 48,
                },
            },
            {
                name: 'Saturday Morning CRT',
                description:
                    'Chunky scale, centered framing, and just enough stars to feel like an old browser tab you never wanted to close.',
                controls: {
                    animationSpeed: 5,
                    catTheme: 'Classic',
                    motionMode: 'Original',
                    positionX: 0,
                    positionY: -6,
                    rainbowMode: 'Original',
                    scale: 128,
                    starDensity: 36,
                },
            },
            {
                name: 'Blueberry Breakfast',
                description:
                    'Same animation grammar, but the pastry look shifts into a bright cereal-box blueberry palette.',
                controls: {
                    animationSpeed: 4,
                    catTheme: 'Blueberry',
                    motionMode: 'Original',
                    positionX: 8,
                    positionY: 4,
                    rainbowMode: 'Wavy',
                    scale: 118,
                    starDensity: 42,
                },
            },
            {
                name: 'Mint Cartridge',
                description:
                    'A softer remix with mint frosting, slower bobbing, and a little extra ribbon wobble in the trail.',
                controls: {
                    animationSpeed: 3,
                    catTheme: 'Mint',
                    motionMode: 'Original',
                    positionX: 16,
                    positionY: 10,
                    rainbowMode: 'Wavy',
                    scale: 130,
                    starDensity: 38,
                },
            },
            {
                name: 'Pocket Meme',
                description:
                    'A tiny fast cat for keyboard layouts that only need a quick streak of chaos instead of the full mural.',
                controls: {
                    animationSpeed: 7,
                    catTheme: 'Classic',
                    motionMode: 'Dash',
                    positionX: -12,
                    positionY: 10,
                    rainbowMode: 'Wavy',
                    scale: 72,
                    starDensity: 62,
                },
            },
            {
                name: 'Hyper Tunnel',
                description:
                    'The faithful cat drops into a louder synth tunnel with party rainbow hues and faster star streaks.',
                controls: {
                    animationSpeed: 8,
                    catTheme: 'Midnight',
                    motionMode: 'Hyper',
                    positionX: 0,
                    positionY: -10,
                    rainbowMode: 'Party',
                    scale: 110,
                    starDensity: 76,
                },
            },
        ],
    },
)
