import type { DrawFn } from '@hypercolor/sdk'
import { canvas, combo, normalizeSpeed, num, scaleContext } from '@hypercolor/sdk'

const NYAN_DESIGN_BASIS = { height: 200, width: 320 } as const
import { CAT_FRAME_SVG_TEMPLATES, CAT_SPRITE_HEIGHT, CAT_SPRITE_WIDTH } from './cat-frames'

type MotionMode = 'Original' | 'Dash' | 'Hyper'
type RainbowMode = 'Original' | 'Wavy' | 'Party'
type BackgroundMode = 'Classic' | 'Twilight' | 'Deep Space' | 'Bubblegum' | 'Mint Night'
type StarMode = 'Classic' | 'Twinkle' | 'Comets' | 'Warp'
type StarColorMode = 'White' | 'Cream' | 'Candy' | 'Mint' | 'Ice' | 'Rainbow'
type CatTheme = 'Classic' | 'Blueberry' | 'Mint' | 'Midnight'
type FaceStyle = 'Classic' | 'Sleepy' | 'Sparkle'

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
    frosting: string
    outline: string
    pastry: string
    sprinkle: string
    white: string
}

interface BackgroundPalette {
    beam: string
    bottom: string
    grid: string
    solid: string
    top: string
}

type CatPaletteKey = keyof CatPalette
type RectSpec = readonly [number, number, number, number]
type TransformMatrix = readonly [number, number, number, number, number, number]

interface CatPoint {
    x: number
    y: number
}

interface CatShape {
    fill: CatPaletteKey
    polygons: readonly (readonly CatPoint[])[]
}

const MOTION_MODES = ['Original', 'Dash', 'Hyper'] as const
const RAINBOW_MODES = ['Original', 'Wavy', 'Party'] as const
const BACKGROUND_MODES = ['Classic', 'Twilight', 'Deep Space', 'Bubblegum', 'Mint Night'] as const
const STAR_MODES = ['Classic', 'Twinkle', 'Comets', 'Warp'] as const
const STAR_COLOR_MODES = ['White', 'Cream', 'Candy', 'Mint', 'Ice', 'Rainbow'] as const
const CAT_THEMES = ['Classic', 'Blueberry', 'Mint', 'Midnight'] as const
const FACE_STYLES = ['Classic', 'Sleepy', 'Sparkle'] as const

const CLASSIC_RAINBOW = ['#ff0000', '#ff9900', '#ffff00', '#33ff00', '#0099ff', '#6633ff'] as const

const CAT_THEMES_PALETTE: Record<CatTheme, CatPalette> = {
    Blueberry: {
        blush: '#ffb0d8',
        frosting: '#8ed8ff',
        fur: '#a2a7c7',
        outline: '#000000',
        pastry: '#dfc6a7',
        sprinkle: '#f7f2ff',
        white: '#ffffff',
    },
    Classic: {
        blush: '#ff9999',
        frosting: '#ff99ff',
        fur: '#999999',
        outline: '#000000',
        pastry: '#ffcc99',
        sprinkle: '#ff3399',
        white: '#ffffff',
    },
    Midnight: {
        blush: '#ff7db0',
        frosting: '#9a78ff',
        fur: '#70707f',
        outline: '#000000',
        pastry: '#bca2d8',
        sprinkle: '#ffe073',
        white: '#ffffff',
    },
    Mint: {
        blush: '#ffb1c7',
        frosting: '#92ffd7',
        fur: '#a9ada4',
        outline: '#000000',
        pastry: '#f3d7a8',
        sprinkle: '#42dca3',
        white: '#ffffff',
    },
}

const BACKGROUND_PALETTES: Record<BackgroundMode, BackgroundPalette> = {
    Bubblegum: {
        beam: '#ff8dd8',
        bottom: '#31124f',
        grid: '#ffd4f3',
        solid: '#542973',
        top: '#7b3f9d',
    },
    Classic: {
        beam: '#ffffff',
        bottom: '#003366',
        grid: '#ffffff',
        solid: '#003366',
        top: '#072859',
    },
    'Deep Space': {
        beam: '#8cd6ff',
        bottom: '#02081f',
        grid: '#8bbcff',
        solid: '#06183d',
        top: '#10244f',
    },
    'Mint Night': {
        beam: '#8fffe1',
        bottom: '#072f33',
        grid: '#cffff4',
        solid: '#0f4950',
        top: '#1a6464',
    },
    Twilight: {
        beam: '#ffd4a8',
        bottom: '#1f204f',
        grid: '#ffe7b8',
        solid: '#31306e',
        top: '#5f4b8b',
    },
}

const STAR_COLOR_VALUES = {
    Candy: '#ff8fd8',
    Cream: '#ffe8b3',
    Ice: '#c9efff',
    Mint: '#9bffd9',
    White: '#ffffff',
} as const satisfies Record<Exclude<StarColorMode, 'Rainbow'>, string>

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
const CSS_CAT_HEIGHT = 122
const SPRITE_SCALE_BASE = 0.55
const CAT_RAINBOW_ANCHOR_X = 8
const FACE_FRAME_LAYOUTS = [
    { blushLeftX: 19, blushRightX: 30, blushY: 13, eyeY: 11, leftEyeX: 21, mouthX: 22, mouthY: 14, rightEyeX: 28 },
    { blushLeftX: 20, blushRightX: 31, blushY: 13, eyeY: 11, leftEyeX: 22, mouthX: 23, mouthY: 14, rightEyeX: 29 },
    { blushLeftX: 20, blushRightX: 31, blushY: 14, eyeY: 12, leftEyeX: 22, mouthX: 23, mouthY: 15, rightEyeX: 29 },
    { blushLeftX: 20, blushRightX: 31, blushY: 14, eyeY: 12, leftEyeX: 22, mouthX: 23, mouthY: 15, rightEyeX: 29 },
    { blushLeftX: 19, blushRightX: 30, blushY: 14, eyeY: 12, leftEyeX: 21, mouthX: 22, mouthY: 15, rightEyeX: 28 },
    { blushLeftX: 19, blushRightX: 30, blushY: 13, eyeY: 11, leftEyeX: 21, mouthX: 22, mouthY: 14, rightEyeX: 28 },
] as const
const IDENTITY_TRANSFORM: TransformMatrix = [1, 0, 0, 1, 0, 0]
const CAT_FILL_ROLES = {
    __BLUSH__: 'blush',
    __FROSTING__: 'frosting',
    __FUR__: 'fur',
    __OUTLINE__: 'outline',
    __PASTRY__: 'pastry',
    __SPRINKLE__: 'sprinkle',
    __WHITE__: 'white',
} as const satisfies Record<string, CatPaletteKey>

let parsedCatFrames: readonly (readonly CatShape[])[] | null = null

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

function hsla(hue: number, saturation: number, lightness: number, alpha: number): string {
    const wrappedHue = ((hue % 360) + 360) % 360
    return `hsla(${wrappedHue}, ${clamp(saturation, 0, 100)}%, ${clamp(lightness, 0, 100)}%, ${clamp(alpha, 0, 1)})`
}

function colorWithAlpha(color: string, alpha: number): string {
    const normalized = color.trim()
    if (!normalized.startsWith('#')) {
        return normalized
    }

    let hex = normalized.slice(1)
    if (hex.length === 3) {
        hex = hex
            .split('')
            .map((part) => `${part}${part}`)
            .join('')
    }

    if (hex.length !== 6) {
        return normalized
    }

    const red = Number.parseInt(hex.slice(0, 2), 16)
    const green = Number.parseInt(hex.slice(2, 4), 16)
    const blue = Number.parseInt(hex.slice(4, 6), 16)
    return `rgba(${red}, ${green}, ${blue}, ${clamp(alpha, 0, 1)})`
}

function parseNumberList(value: string): number[] {
    return value
        .split(/[,\s]+/)
        .map((part) => part.trim())
        .filter((part) => part.length > 0)
        .map((part) => Number(part))
        .filter((part) => Number.isFinite(part))
}

function multiplyTransform(parent: TransformMatrix, child: TransformMatrix): TransformMatrix {
    return [
        parent[0] * child[0] + parent[2] * child[1],
        parent[1] * child[0] + parent[3] * child[1],
        parent[0] * child[2] + parent[2] * child[3],
        parent[1] * child[2] + parent[3] * child[3],
        parent[0] * child[4] + parent[2] * child[5] + parent[4],
        parent[1] * child[4] + parent[3] * child[5] + parent[5],
    ]
}

function parseTransform(transform: string | null): TransformMatrix {
    if (!transform) {
        return IDENTITY_TRANSFORM
    }

    let combined = IDENTITY_TRANSFORM
    const pattern = /(matrix|translate)\(([^)]+)\)/g

    for (const match of transform.matchAll(pattern)) {
        const [, kind, rawValue] = match
        const values = parseNumberList(rawValue)
        let next = IDENTITY_TRANSFORM

        if (kind === 'translate') {
            next = [1, 0, 0, 1, values[0] ?? 0, values[1] ?? 0]
        } else if (kind === 'matrix' && values.length === 6) {
            next = [values[0], values[1], values[2], values[3], values[4], values[5]]
        }

        combined = multiplyTransform(combined, next)
    }

    return combined
}

function snapCoord(value: number): number {
    const rounded = Math.round(value)
    return Math.abs(value - rounded) < 0.0001 ? rounded : value
}

function applyTransform(point: CatPoint, transform: TransformMatrix): CatPoint {
    return {
        x: snapCoord(transform[0] * point.x + transform[2] * point.y + transform[4]),
        y: snapCoord(transform[1] * point.x + transform[3] * point.y + transform[5]),
    }
}

function parsePathPolygons(pathData: string): readonly (readonly CatPoint[])[] {
    const tokens = pathData.match(/[MmZz]|-?\d*\.?\d+(?:e[-+]?\d+)?/g)
    if (!tokens) {
        return []
    }

    const polygons: CatPoint[][] = []
    let currentX = 0
    let currentY = 0
    let startX = 0
    let startY = 0
    let activePolygon: CatPoint[] | null = null
    let index = 0
    let command = ''

    const closePolygon = (): void => {
        if (activePolygon && activePolygon.length >= 3) {
            polygons.push(activePolygon)
        }
        activePolygon = null
    }

    while (index < tokens.length) {
        const token = tokens[index] ?? ''
        if (/^[MmZz]$/.test(token)) {
            command = token
            index += 1
        }

        if (command === 'z' || command === 'Z') {
            closePolygon()
            currentX = startX
            currentY = startY
            continue
        }

        if (command !== 'm' && command !== 'M') {
            break
        }

        const isRelative = command === 'm'
        const moveX = Number(tokens[index] ?? 0)
        const moveY = Number(tokens[index + 1] ?? 0)
        index += 2

        if (activePolygon) {
            closePolygon()
        }

        currentX = isRelative ? currentX + moveX : moveX
        currentY = isRelative ? currentY + moveY : moveY
        startX = currentX
        startY = currentY
        activePolygon = [{ x: currentX, y: currentY }]

        while (index + 1 < tokens.length && !/^[MmZz]$/.test(tokens[index] ?? '')) {
            const nextX = Number(tokens[index] ?? 0)
            const nextY = Number(tokens[index + 1] ?? 0)
            index += 2

            currentX = isRelative ? currentX + nextX : nextX
            currentY = isRelative ? currentY + nextY : nextY
            activePolygon.push({ x: currentX, y: currentY })
        }
    }

    if (activePolygon && activePolygon.length >= 3) {
        polygons.push(activePolygon)
    }

    return polygons
}

function resolvePathFill(path: Element): CatPaletteKey | null {
    const style = path.getAttribute('style') ?? ''
    const fillMatch = style.match(/(?:^|;)\s*fill:\s*([^;]+)/)
    const fillValue = fillMatch?.[1]?.trim() ?? path.getAttribute('fill')?.trim()
    if (!fillValue) {
        return null
    }

    return CAT_FILL_ROLES[fillValue as keyof typeof CAT_FILL_ROLES] ?? null
}

function collectCatShapes(element: Element, inheritedTransform: TransformMatrix, shapes: CatShape[]): void {
    const combinedTransform = multiplyTransform(inheritedTransform, parseTransform(element.getAttribute('transform')))

    if (element.tagName.toLowerCase() === 'path') {
        const fill = resolvePathFill(element)
        const pathData = element.getAttribute('d')
        if (fill && pathData) {
            const polygons = parsePathPolygons(pathData)
                .map((polygon) => polygon.map((point) => applyTransform(point, combinedTransform)))
                .filter((polygon) => polygon.length >= 3)

            if (polygons.length > 0) {
                shapes.push({ fill, polygons })
            }
        }
    }

    for (const child of element.children) {
        collectCatShapes(child, combinedTransform, shapes)
    }
}

function getParsedCatFrames(): readonly (readonly CatShape[])[] {
    if (parsedCatFrames) {
        return parsedCatFrames
    }

    if (typeof DOMParser === 'undefined') {
        return []
    }

    const parser = new DOMParser()
    parsedCatFrames = CAT_FRAME_SVG_TEMPLATES.map((svgTemplate) => {
        const doc = parser.parseFromString(svgTemplate, 'image/svg+xml')
        const root = doc.documentElement
        const shapes: CatShape[] = []
        collectCatShapes(root, IDENTITY_TRANSFORM, shapes)
        return shapes
    })

    return parsedCatFrames
}

function buildCatFrameCanvas(shapes: readonly CatShape[], palette: CatPalette): HTMLCanvasElement | null {
    if (typeof document === 'undefined') {
        return null
    }

    const frameCanvas = document.createElement('canvas')
    frameCanvas.width = CAT_SPRITE_WIDTH
    frameCanvas.height = CAT_SPRITE_HEIGHT

    const frameCtx = frameCanvas.getContext('2d')
    if (!frameCtx) {
        return null
    }

    frameCtx.imageSmoothingEnabled = false

    for (const shape of shapes) {
        frameCtx.fillStyle = palette[shape.fill]
        frameCtx.beginPath()

        for (const polygon of shape.polygons) {
            const [firstPoint, ...rest] = polygon
            if (!firstPoint) continue

            frameCtx.moveTo(firstPoint.x, firstPoint.y)
            for (const point of rest) {
                frameCtx.lineTo(point.x, point.y)
            }
            frameCtx.closePath()
        }

        frameCtx.fill()
    }

    return frameCanvas
}

function buildThemedCatFrames(palette: CatPalette): readonly HTMLCanvasElement[] {
    return getParsedCatFrames()
        .map((shapes) => buildCatFrameCanvas(shapes, palette))
        .filter((frameCanvas): frameCanvas is HTMLCanvasElement => frameCanvas !== null)
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

function pickBackgroundMode(value: string): BackgroundMode {
    return BACKGROUND_MODES.includes(value as BackgroundMode) ? (value as BackgroundMode) : 'Classic'
}

function pickStarMode(value: string): StarMode {
    return STAR_MODES.includes(value as StarMode) ? (value as StarMode) : 'Classic'
}

function pickStarColorMode(value: string): StarColorMode {
    return STAR_COLOR_MODES.includes(value as StarColorMode) ? (value as StarColorMode) : 'White'
}

function pickCatTheme(value: string): CatTheme {
    return CAT_THEMES.includes(value as CatTheme) ? (value as CatTheme) : 'Classic'
}

function pickFaceStyle(value: string): FaceStyle {
    return FACE_STYLES.includes(value as FaceStyle) ? (value as FaceStyle) : 'Classic'
}

function rainbowColor(index: number, mode: RainbowMode, time: number): string {
    if (mode === 'Party') {
        return hsl(time * 55 + index * 44, 95, 58)
    }

    return CLASSIC_RAINBOW[index] ?? CLASSIC_RAINBOW[0]
}

function starColor(starColorMode: StarColorMode, seed: number, time: number, alpha = 1): string {
    if (starColorMode === 'Rainbow') {
        return alpha >= 1 ? hsl(time * 80 + seed * 23, 92, 84) : hsla(time * 80 + seed * 23, 92, 84, alpha)
    }

    const base = STAR_COLOR_VALUES[starColorMode]
    return alpha >= 1 ? base : colorWithAlpha(base, alpha)
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

function drawFaceStyle(
    ctx: CanvasRenderingContext2D,
    faceStyle: FaceStyle,
    palette: CatPalette,
    spriteX: number,
    spriteY: number,
    spriteScale: number,
    frameIndex: number,
): void {
    if (faceStyle === 'Classic') {
        return
    }

    const layout = FACE_FRAME_LAYOUTS[frameIndex] ?? FACE_FRAME_LAYOUTS[0]
    if (faceStyle === 'Sleepy') {
        drawScaledRects(
            ctx,
            spriteX,
            spriteY,
            spriteScale,
            [
                [layout.leftEyeX - 1, layout.eyeY, 3, 2],
                [layout.rightEyeX - 1, layout.eyeY, 3, 2],
            ],
            palette.fur,
        )
        drawScaledRects(
            ctx,
            spriteX,
            spriteY,
            spriteScale,
            [
                [layout.leftEyeX - 1, layout.eyeY + 1, 3, 1],
                [layout.rightEyeX - 1, layout.eyeY + 1, 3, 1],
            ],
            palette.outline,
        )
        return
    }

    drawScaledRects(
        ctx,
        spriteX,
        spriteY,
        spriteScale,
        [
            [layout.leftEyeX - 2, layout.eyeY, 1, 1],
            [layout.leftEyeX - 3, layout.eyeY + 1, 1, 1],
            [layout.leftEyeX - 2, layout.eyeY + 2, 1, 1],
            [layout.leftEyeX - 1, layout.eyeY + 1, 1, 1],
            [layout.rightEyeX + 2, layout.eyeY, 1, 1],
            [layout.rightEyeX + 3, layout.eyeY + 1, 1, 1],
            [layout.rightEyeX + 2, layout.eyeY + 2, 1, 1],
            [layout.rightEyeX + 1, layout.eyeY + 1, 1, 1],
        ],
        palette.white,
    )
    drawScaledRects(
        ctx,
        spriteX,
        spriteY,
        spriteScale,
        [
            [layout.leftEyeX - 2, layout.eyeY + 1, 1, 1],
            [layout.rightEyeX + 2, layout.eyeY + 1, 1, 1],
        ],
        palette.sprinkle,
    )
}

function drawBackground(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    motionMode: MotionMode,
    backgroundMode: BackgroundMode,
    time: number,
): void {
    const palette = BACKGROUND_PALETTES[backgroundMode]

    if (motionMode === 'Hyper') {
        const gradient = ctx.createLinearGradient(0, 0, 0, height)
        gradient.addColorStop(0, palette.top)
        gradient.addColorStop(1, palette.bottom)
        ctx.fillStyle = gradient
    } else {
        ctx.fillStyle = palette.solid
    }

    ctx.fillRect(0, 0, width, height)

    if (motionMode === 'Hyper') {
        ctx.fillStyle = colorWithAlpha(palette.grid, 0.07)
        for (let line = 0; line < height; line += 12) {
            ctx.fillRect(0, line, width, 2)
        }

        ctx.fillStyle = colorWithAlpha(palette.beam, 0.08)
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

function drawClassicStars(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    time: number,
    speed: number,
    starColorMode: StarColorMode,
    starDensity: number,
    cssScale: number,
): void {
    const cycle = ((time * speed) / 0.7) % 1
    const loopWidth = 400 * cssScale
    const scroll = (((time * speed * 400) / 0.7) * cssScale) % loopWidth
    const sparkWidth = 40 * cssScale
    const rowHeight = 300 * cssScale
    const opacity = clamp(starDensity / 60, 0, 1)

    ctx.save()
    ctx.globalAlpha = opacity

    for (let row = -rowHeight; row < height + rowHeight; row += rowHeight) {
        for (let trackX = width - scroll - loopWidth; trackX < width + loopWidth; trackX += loopWidth) {
            for (const [baseX, baseY, phaseOffset] of CSS_SPARK_BASES) {
                const phase = (cycle + phaseOffset) % 1
                const phaseIndex =
                    phase < 0.16 ? 0 : phase < 0.33 ? 1 : phase < 0.5 ? 2 : phase < 0.66 ? 3 : phase < 0.83 ? 4 : 5
                const sparkX = trackX + baseX * cssScale
                if (sparkX <= -sparkWidth || sparkX >= width + sparkWidth) continue

                const sparkY = row + baseY * cssScale
                const color = starColor(starColorMode, baseX + baseY + phaseIndex * 17, time)
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
    }

    ctx.restore()
}

function drawTwinkleField(
    ctx: CanvasRenderingContext2D,
    stars: Star[],
    width: number,
    height: number,
    time: number,
    speed: number,
    starColorMode: StarColorMode,
    starDensity: number,
    cssScale: number,
): void {
    const loopWidth = width + 80 * cssScale
    const unit = Math.max(1, Math.round(cssScale))

    ctx.save()
    ctx.globalAlpha = clamp(0.35 + starDensity / 120, 0.25, 1)

    for (const [index, seed] of stars.entries()) {
        const scroll = time * seed.speed * speed * 0.55
        const x = ((((seed.x * loopWidth - scroll) % loopWidth) + loopWidth) % loopWidth) - 20 * cssScale
        const y = 12 + seed.y * (height - 24) + Math.sin(time * 0.9 + seed.drift) * 3 * cssScale
        const frame = Math.floor(time * seed.twinkle * 2 + seed.phase) % 4
        const color = starColor(starColorMode, index, time)

        drawTwinkle(ctx, x, y, unit, frame, seed.size, color)
    }

    ctx.restore()
}

function drawCometStars(
    ctx: CanvasRenderingContext2D,
    stars: Star[],
    width: number,
    height: number,
    time: number,
    speed: number,
    starColorMode: StarColorMode,
    starDensity: number,
    cssScale: number,
): void {
    drawTwinkleField(ctx, stars, width, height, time, speed, starColorMode, starDensity * 0.85, cssScale)

    const cometCount = Math.max(2, Math.round(starDensity / 28))
    const unit = Math.max(1, Math.round(cssScale))

    ctx.save()
    ctx.globalAlpha = clamp(0.45 + starDensity / 140, 0.35, 1)

    for (let index = 0; index < cometCount; index++) {
        const loopWidth = width + 220 * cssScale
        const cometSpeed = (80 + index * 22) * speed * cssScale
        const headX = width + 60 * cssScale - ((time * cometSpeed + index * 96 * cssScale) % loopWidth)
        const lane = (index + 1) / (cometCount + 1)
        const headY = height * (0.12 + lane * 0.76) + Math.sin(time * 0.7 + index * 1.9) * 8 * cssScale
        const tailLength = (28 + index * 7) * cssScale
        const color = starColor(starColorMode, index * 2 + 11, time)
        const tail = ctx.createLinearGradient(headX - tailLength, headY, headX, headY)
        tail.addColorStop(0, 'rgba(255, 255, 255, 0)')
        tail.addColorStop(1, color)

        ctx.fillStyle = tail
        ctx.fillRect(
            Math.round(headX - tailLength),
            Math.round(headY - unit / 2),
            Math.max(1, Math.round(tailLength)),
            Math.max(1, unit),
        )
        drawTwinkle(ctx, headX - unit * 1.5, headY - unit * 1.5, unit, 3, 1, color)
    }

    ctx.restore()
}

function drawWarpStars(
    ctx: CanvasRenderingContext2D,
    stars: Star[],
    width: number,
    height: number,
    time: number,
    speed: number,
    starColorMode: StarColorMode,
    starDensity: number,
    cssScale: number,
): void {
    const loopWidth = width + 80

    ctx.save()
    ctx.globalAlpha = clamp(0.4 + starDensity / 110, 0.25, 1)

    for (const [index, seed] of stars.entries()) {
        const scroll = time * seed.speed * speed * 2.4
        const x = ((((seed.x * loopWidth - scroll) % loopWidth) + loopWidth) % loopWidth) - 20
        const y = 12 + seed.y * (height - 24) + Math.sin(time * 0.9 + seed.drift) * 3
        const frame = Math.floor(time * seed.twinkle * 2 + seed.phase) % 4
        const color = starColor(starColorMode, index, time)

        drawTwinkle(ctx, x, y, seed.size, frame, seed.size, color)
    }

    ctx.strokeStyle = starColor(starColorMode, 99, time, 0.28)
    ctx.lineWidth = Math.max(1, cssScale)

    for (const [index, base] of STAR_OFFSETS.entries()) {
        const offset = ((time * 220 * speed + base[0]) % (width + 120)) - 60
        const y = 28 + base[1] * (height / 220)
        ctx.beginPath()
        ctx.moveTo(width - offset, y)
        ctx.lineTo(width - offset - 42 - index * 8, y)
        ctx.stroke()
    }

    ctx.restore()
}

function drawStars(
    ctx: CanvasRenderingContext2D,
    stars: Star[],
    width: number,
    height: number,
    time: number,
    speed: number,
    starMode: StarMode,
    starColorMode: StarColorMode,
    starDensity: number,
    cssScale: number,
): void {
    if (starMode === 'Classic') {
        drawClassicStars(ctx, width, height, time, speed, starColorMode, starDensity, cssScale)
        return
    }

    if (starMode === 'Comets') {
        drawCometStars(ctx, stars, width, height, time, speed, starColorMode, starDensity, cssScale)
        return
    }

    if (starMode === 'Warp') {
        drawWarpStars(ctx, stars, width, height, time, speed, starColorMode, starDensity, cssScale)
        return
    }

    drawTwinkleField(ctx, stars, width, height, time, speed, starColorMode, starDensity, cssScale)
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
        backgroundMode: combo('Background', [...BACKGROUND_MODES], { group: 'Atmosphere' }),
        rainbowMode: combo('Rainbow', [...RAINBOW_MODES], { group: 'Style' }),
        catTheme: combo('Cat Theme', [...CAT_THEMES], { group: 'Style' }),
        faceStyle: combo('Face', [...FACE_STYLES], { group: 'Style' }),
        animationSpeed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        scale: num('Scale', [50, 180], 100, { group: 'Layout' }),
        positionX: num('Position X', [-100, 100], 0, { group: 'Layout' }),
        positionY: num('Position Y', [-100, 100], 0, { group: 'Layout' }),
        starMode: combo('Stars', [...STAR_MODES], { group: 'Atmosphere' }),
        starColorMode: combo('Star Color', [...STAR_COLOR_MODES], { group: 'Atmosphere' }),
        starDensity: num('Star Density', [0, 100], 50, { group: 'Atmosphere' }),
    },
    () => {
        let stars: Star[] = []
        let starCount = -1
        const themedCatFrames = {
            Blueberry: buildThemedCatFrames(CAT_THEMES_PALETTE.Blueberry),
            Classic: buildThemedCatFrames(CAT_THEMES_PALETTE.Classic),
            Midnight: buildThemedCatFrames(CAT_THEMES_PALETTE.Midnight),
            Mint: buildThemedCatFrames(CAT_THEMES_PALETTE.Mint),
        } satisfies Record<CatTheme, readonly HTMLCanvasElement[]>

        const render: DrawFn = (ctx, time, controls) => {
            const width = ctx.canvas.width
            const height = ctx.canvas.height
            const speed = normalizeSpeed(controls.animationSpeed as number)
            const scaleControl = clamp(controls.scale as number, 50, 180)
            const positionX = clamp(controls.positionX as number, -100, 100)
            const positionY = clamp(controls.positionY as number, -100, 100)
            const starDensity = clamp(controls.starDensity as number, 0, 100)
            const motionMode = pickMotionMode(String(controls.motionMode))
            const backgroundMode = pickBackgroundMode(String(controls.backgroundMode))
            const rainbowMode = pickRainbowMode(String(controls.rainbowMode))
            const starMode = pickStarMode(String(controls.starMode))
            const starColorMode = pickStarColorMode(String(controls.starColorMode))
            const catTheme = pickCatTheme(String(controls.catTheme))
            const faceStyle = pickFaceStyle(String(controls.faceStyle))
            const catPalette = CAT_THEMES_PALETTE[catTheme]

            const nextStarCount = Math.floor(starDensity * 1.4)
            if (nextStarCount !== starCount) {
                starCount = nextStarCount
                stars = buildStars(starCount)
            }

            const sceneScale = Math.max(0.75, scaleContext({ height, width }, NYAN_DESIGN_BASIS).scale)
            const unit = Math.max(1, Math.round(6 * SPRITE_SCALE_BASE * sceneScale * (scaleControl / 100)))
            const cssScale = unit / 6
            const spriteScale = (CSS_CAT_HEIGHT * cssScale) / CAT_SPRITE_HEIGHT
            const spriteWidth = CAT_SPRITE_WIDTH * spriteScale
            const spriteHeight = CAT_SPRITE_HEIGHT * spriteScale
            const frameIndex = Math.floor(time * CAT_FRAME_RATE * speed) % CAT_FRAME_SVG_TEMPLATES.length

            drawBackground(ctx, width, height, motionMode, backgroundMode, time)
            drawStars(ctx, stars, width, height, time, speed, starMode, starColorMode, starDensity, cssScale)

            const offsetX = (positionX / 100) * width * 0.36
            const offsetY = (positionY / 100) * height * 0.34

            let centerX = width * 0.5 + offsetX
            if (motionMode === 'Dash') {
                const travel = ((time * speed * 42) % (width + spriteWidth * 1.6)) - spriteWidth * 0.8
                centerX = travel + offsetX
            } else if (motionMode === 'Hyper') {
                centerX += Math.sin(time * speed * 1.5) * unit * 1.2
            }

            const centerY = clamp(height * 0.52 + offsetY, spriteHeight * 0.25, height - spriteHeight * 0.2)
            const trailEnd = centerX - spriteWidth / 2 + CAT_RAINBOW_ANCHOR_X * spriteScale

            drawRainbow(ctx, trailEnd, centerY, cssScale, time, speed, rainbowMode)
            const catFrame = themedCatFrames[catTheme][frameIndex]
            if (catFrame) {
                const spriteX = Math.round(centerX - spriteWidth / 2)
                const spriteY = Math.round(centerY - spriteHeight / 2)
                ctx.imageSmoothingEnabled = false
                ctx.drawImage(
                    catFrame,
                    spriteX,
                    spriteY,
                    Math.max(1, Math.round(spriteWidth)),
                    Math.max(1, Math.round(spriteHeight)),
                )
                drawFaceStyle(ctx, faceStyle, catPalette, spriteX, spriteY, spriteScale, frameIndex)
            }
        }

        return render
    },
    {
        description:
            'A tiny pop-tart space cat with sweet presets, playful faces, and stars that can go from cozy to gloriously over the top.',
        designBasis: NYAN_DESIGN_BASIS,
        presets: [
            {
                name: 'Original Loop',
                description:
                    'The faithful one: classic cat, classic face, classic stars, and the familiar little rainbow march.',
                controls: {
                    animationSpeed: 5,
                    backgroundMode: 'Classic',
                    catTheme: 'Classic',
                    faceStyle: 'Classic',
                    motionMode: 'Original',
                    positionX: 0,
                    positionY: 0,
                    rainbowMode: 'Original',
                    scale: 100,
                    starColorMode: 'White',
                    starMode: 'Classic',
                    starDensity: 52,
                },
            },
            {
                name: 'Comet Dash',
                description:
                    'The cat zips across a deeper sky while cool comet tails streak past in front of the rainbow.',
                controls: {
                    animationSpeed: 6,
                    backgroundMode: 'Deep Space',
                    catTheme: 'Classic',
                    faceStyle: 'Classic',
                    motionMode: 'Dash',
                    positionX: -10,
                    positionY: -8,
                    rainbowMode: 'Original',
                    scale: 96,
                    starColorMode: 'Ice',
                    starMode: 'Comets',
                    starDensity: 56,
                },
            },
            {
                name: 'Blueberry Sparkle',
                description: 'Blue frosting, bright glints, and a dreamy twilight sky with soft pastel stars.',
                controls: {
                    animationSpeed: 4,
                    backgroundMode: 'Twilight',
                    catTheme: 'Blueberry',
                    faceStyle: 'Sparkle',
                    motionMode: 'Original',
                    positionX: 6,
                    positionY: 2,
                    rainbowMode: 'Wavy',
                    scale: 116,
                    starColorMode: 'Cream',
                    starMode: 'Twinkle',
                    starDensity: 44,
                },
            },
            {
                name: 'Mint Moonbeam',
                description:
                    'Slow, dreamy, and a little sleepy, like Nyan Cat drifting through the nicest possible bedtime sky.',
                controls: {
                    animationSpeed: 3,
                    backgroundMode: 'Mint Night',
                    catTheme: 'Mint',
                    faceStyle: 'Sleepy',
                    motionMode: 'Original',
                    positionX: 10,
                    positionY: 8,
                    rainbowMode: 'Wavy',
                    scale: 126,
                    starColorMode: 'Mint',
                    starMode: 'Twinkle',
                    starDensity: 34,
                },
            },
            {
                name: 'Midnight Meteors',
                description:
                    'Dark velvet cat, comet traffic, and a cooler palette that still feels sweet instead of moody.',
                controls: {
                    animationSpeed: 5,
                    backgroundMode: 'Deep Space',
                    catTheme: 'Midnight',
                    faceStyle: 'Classic',
                    motionMode: 'Original',
                    positionX: 4,
                    positionY: -4,
                    rainbowMode: 'Wavy',
                    scale: 114,
                    starColorMode: 'Ice',
                    starMode: 'Comets',
                    starDensity: 58,
                },
            },
            {
                name: 'Sleepy Drift',
                description: 'A smaller, floatier loop with sleepy eyes, warm stars, and a soft bubblegum night sky.',
                controls: {
                    animationSpeed: 4,
                    backgroundMode: 'Bubblegum',
                    catTheme: 'Classic',
                    faceStyle: 'Sleepy',
                    motionMode: 'Original',
                    positionX: -6,
                    positionY: 6,
                    rainbowMode: 'Wavy',
                    scale: 86,
                    starColorMode: 'Candy',
                    starMode: 'Twinkle',
                    starDensity: 40,
                },
            },
            {
                name: 'Rainbow Afterparty',
                description:
                    'Full sugar rush: party rainbow, sparkle eyes, candy sky, and warp stars with zero self-control.',
                controls: {
                    animationSpeed: 8,
                    backgroundMode: 'Bubblegum',
                    catTheme: 'Midnight',
                    faceStyle: 'Sparkle',
                    motionMode: 'Hyper',
                    positionX: 0,
                    positionY: -10,
                    rainbowMode: 'Party',
                    scale: 110,
                    starColorMode: 'Rainbow',
                    starMode: 'Warp',
                    starDensity: 76,
                },
            },
        ],
    },
)
