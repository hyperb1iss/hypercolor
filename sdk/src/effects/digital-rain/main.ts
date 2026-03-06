import { canvas, combo } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface Rgb { r: number; g: number; b: number }

interface RainPalette {
    background: string
    shadow: Rgb
    trail: Rgb
    bright: Rgb
    head: Rgb
    glitch: Rgb
}

interface ColumnState {
    head: number
    speedBias: number
    trailBias: number
    active: boolean
    respawnGap: number
    glyphs: number[]
    mutateClock: number
    mutateEvery: number
}

// ── Constants ────────────────────────────────────────────────────────────

const COLOR_MODES = ['Matrix', 'Phosphor', 'SilkCircuit', 'Cyberpunk', 'Ice'] as const

const GLYPHS = [
    '█', '▓', '▒', '░', '▉', '▊', '▋', '▌', '▀', '▄',
    '■', '▣', '╳', '╱', '╲',
]

const PALETTES: Record<string, RainPalette> = {
    Matrix: {
        background: '#020903',
        shadow: { r: 5, g: 28, b: 10 },
        trail: { r: 42, g: 138, b: 56 },
        bright: { r: 138, g: 248, b: 162 },
        head: { r: 184, g: 255, b: 202 },
        glitch: { r: 182, g: 255, b: 196 },
    },
    Phosphor: {
        background: '#120903',
        shadow: { r: 32, g: 18, b: 4 },
        trail: { r: 156, g: 102, b: 32 },
        bright: { r: 250, g: 185, b: 88 },
        head: { r: 255, g: 196, b: 124 },
        glitch: { r: 255, g: 154, b: 72 },
    },
    SilkCircuit: {
        background: '#06060f',
        shadow: { r: 20, g: 12, b: 40 },
        trail: { r: 128, g: 255, b: 234 },
        bright: { r: 225, g: 53, b: 255 },
        head: { r: 162, g: 255, b: 244 },
        glitch: { r: 255, g: 106, b: 193 },
    },
    Cyberpunk: {
        background: '#08020f',
        shadow: { r: 22, g: 8, b: 32 },
        trail: { r: 255, g: 106, b: 193 },
        bright: { r: 132, g: 245, b: 255 },
        head: { r: 204, g: 226, b: 255 },
        glitch: { r: 255, g: 154, b: 72 },
    },
    Ice: {
        background: '#010917',
        shadow: { r: 6, g: 22, b: 40 },
        trail: { r: 62, g: 160, b: 232 },
        bright: { r: 146, g: 228, b: 255 },
        head: { r: 182, g: 238, b: 255 },
        glitch: { r: 162, g: 252, b: 255 },
    },
}

// ── Helpers ──────────────────────────────────────────────────────────────

function clamp(value: number, min: number, max: number): number {
    if (value < min) return min
    if (value > max) return max
    return value
}

function randomRange(min: number, max: number): number {
    return min + Math.random() * (max - min)
}

function randomGlyphIndex(): number {
    return Math.floor(Math.random() * GLYPHS.length)
}

function rgba(color: Rgb, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const blend = clamp(t, 0, 1)
    return {
        r: Math.round(a.r + (b.r - a.r) * blend),
        g: Math.round(a.g + (b.g - a.g) * blend),
        b: Math.round(a.b + (b.b - a.b) * blend),
    }
}

function resolvePalette(mode: string): RainPalette {
    return PALETTES[mode] ?? PALETTES.Matrix
}

function wrapRow(row: number, rows: number): number {
    if (rows <= 0) return 0
    const wrapped = row % rows
    return wrapped < 0 ? wrapped + rows : wrapped
}

function activeProbability(density: number): number {
    const normalized = clamp((density - 10) / 90, 0, 1)
    return 0.14 + normalized * 0.84
}

function columnTrailCells(column: ColumnState, trailLength: number, density: number): number {
    const baseTrail = 4 + trailLength * 0.26
    const densityBoost = 0.86 + activeProbability(density) * 0.24
    return Math.max(4, Math.round(baseTrail * column.trailBias * densityBoost))
}

function createColumn(rows: number, seedHead: boolean, density: number): ColumnState {
    const glyphs = new Array<number>(rows)
    for (let i = 0; i < rows; i++) glyphs[i] = randomGlyphIndex()

    return {
        head: seedHead ? randomRange(-rows, rows) : -randomRange(2, rows * 1.2 + 2),
        speedBias: randomRange(0.62, 1.52),
        trailBias: randomRange(0.72, 1.28),
        active: Math.random() < activeProbability(density),
        respawnGap: randomRange(2, 8),
        glyphs,
        mutateClock: Math.random() * 0.2,
        mutateEvery: randomRange(0.03, 0.16),
    }
}

function resetColumn(column: ColumnState, rows: number, density: number, forceActive: boolean): void {
    column.active = forceActive || Math.random() < activeProbability(density)
    column.head = -randomRange(2, rows * (0.4 + Math.random() * 0.9))
    column.speedBias = randomRange(0.6, 1.58)
    column.trailBias = randomRange(0.7, 1.35)
    column.respawnGap = randomRange(2, 9)
    column.mutateClock = Math.random() * 0.25
    column.mutateEvery = randomRange(0.03, 0.18)

    if (column.glyphs.length !== rows) {
        column.glyphs = new Array<number>(rows)
        for (let i = 0; i < rows; i++) column.glyphs[i] = randomGlyphIndex()
        return
    }

    for (let i = 0; i < rows; i++) {
        if (Math.random() < 0.34) {
            column.glyphs[i] = randomGlyphIndex()
        }
    }
}

function mutateColumnGlyphs(column: ColumnState, rows: number, trailLength: number, density: number): void {
    const mutationCount = 1 + (Math.random() < 0.4 ? 1 : 0)
    const headRow = Math.floor(column.head)

    for (let i = 0; i < mutationCount; i++) {
        const offset = Math.floor(Math.random() * columnTrailCells(column, trailLength, density))
        const jitter = Math.floor((Math.random() - 0.5) * 6)
        const row = wrapRow(headRow - offset + jitter, rows)
        column.glyphs[row] = randomGlyphIndex()
    }

    if (Math.random() < 0.12) {
        const randomRow = Math.floor(Math.random() * rows)
        column.glyphs[randomRow] = randomGlyphIndex()
    }
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful('Digital Rain', {
    speed:     [1, 10, 5],
    density:   [10, 100, 62],
    trailLength: [5, 100, 58],
    charSize:  [0, 100, 54],
    colorMode: combo('Color Mode', [...COLOR_MODES], { default: 'Matrix' }),
    glitch:    false,
}, () => {
    let columns: ColumnState[] = []
    let rows = 0
    let cols = 0
    let cellWidth = 10
    let cellHeight = 16
    let lastCanvasWidth = 0
    let lastCanvasHeight = 0
    let needsGridSync = true
    let paletteState: RainPalette = PALETTES.Matrix
    let lastTime = -1

    let prevCharSize = 54
    let prevColorMode = 'Matrix'

    function rebuildColumns(density: number): void {
        columns = []
        for (let i = 0; i < cols; i++) {
            columns.push(createColumn(rows, true, density))
        }
    }

    function syncGrid(w: number, h: number, charSize: number, density: number): void {
        const nextCellWidth = Math.max(8, Math.round(8 + charSize * 0.16))
        const nextCellHeight = Math.max(nextCellWidth + 4, Math.round(nextCellWidth * 1.7))
        const nextCols = Math.max(8, Math.floor(w / nextCellWidth))
        const nextRows = Math.max(8, Math.floor(h / nextCellHeight))

        const canvasChanged = lastCanvasWidth !== w || lastCanvasHeight !== h
        const gridChanged =
            cellWidth !== nextCellWidth ||
            cellHeight !== nextCellHeight ||
            cols !== nextCols ||
            rows !== nextRows

        if (!needsGridSync && !canvasChanged && !gridChanged) return

        lastCanvasWidth = w
        lastCanvasHeight = h
        cellWidth = nextCellWidth
        cellHeight = nextCellHeight
        cols = nextCols
        rows = nextRows
        rebuildColumns(density)
        needsGridSync = false
    }

    function updateColumns(dt: number, speed: number, density: number, trailLength: number): void {
        const rowsPerSecond = 3.5 + speed * 8.2
        const wakeChance = dt * (0.14 + density * 0.004)

        for (const column of columns) {
            if (!column.active) {
                if (Math.random() < wakeChance * (0.35 + activeProbability(density))) {
                    resetColumn(column, rows, density, true)
                }
                continue
            }

            column.head += rowsPerSecond * column.speedBias * dt

            const trailCells = columnTrailCells(column, trailLength, density)
            if (column.head - trailCells > rows + column.respawnGap) {
                resetColumn(column, rows, density, false)
                continue
            }

            column.mutateClock += dt
            if (column.mutateClock >= column.mutateEvery) {
                column.mutateClock = 0
                mutateColumnGlyphs(column, rows, trailLength, density)
            }
        }
    }

    function drawAtmosphere(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        trailLength: number,
        density: number,
        glitch: boolean,
    ): void {
        const haze = ctx.createLinearGradient(0, 0, 0, h)
        haze.addColorStop(0, rgba(paletteState.shadow, 0.28))
        haze.addColorStop(0.58, rgba(paletteState.shadow, 0.1))
        haze.addColorStop(1, rgba(paletteState.shadow, 0.02))
        ctx.fillStyle = haze
        ctx.fillRect(0, 0, w, h)

        const scanlineAlpha = 0.02 + (trailLength / 100) * 0.05
        ctx.fillStyle = rgba(paletteState.trail, scanlineAlpha)
        for (let y = 1; y < h; y += 3) {
            ctx.fillRect(0, y, w, 1)
        }

        if (!glitch || Math.random() >= 0.14) return

        const bandY = Math.floor(Math.random() * h)
        const bandHeight = 1 + Math.floor(Math.random() * 3)
        ctx.fillStyle = rgba(paletteState.glitch, 0.08 + Math.random() * 0.2)
        ctx.fillRect(0, bandY, w, bandHeight)
    }

    function drawDormantGlyph(ctx: CanvasRenderingContext2D, columnIndex: number, density: number): void {
        if (Math.random() >= 0.01 + (density / 100) * 0.02) return

        const row = Math.floor(Math.random() * rows)
        const glyph = GLYPHS[randomGlyphIndex()]
        const x = columnIndex * cellWidth
        const y = row * cellHeight
        ctx.fillStyle = rgba(paletteState.trail, 0.035)
        ctx.fillText(glyph, x, y)
    }

    function drawColumnTrail(
        ctx: CanvasRenderingContext2D,
        column: ColumnState,
        columnIndex: number,
        glitchPulse: number,
        trailLength: number,
        density: number,
        glitch: boolean,
    ): void {
        const trailCells = columnTrailCells(column, trailLength, density)
        const headRow = Math.floor(column.head)
        const x = columnIndex * cellWidth

        for (let step = 0; step < trailCells; step++) {
            const row = headRow - step
            if (row < 0 || row >= rows) continue

            const energy = 1 - step / trailCells
            const fade = Math.pow(energy, 1.6)
            if (fade < 0.02) continue

            const glyph = GLYPHS[column.glyphs[row] ?? 0]
            const y = row * cellHeight
            let color = mixRgb(paletteState.trail, paletteState.bright, Math.pow(energy, 0.58))
            let alpha = (0.12 + trailLength / 135) * fade

            if (step === 0) {
                alpha = 0.92
                color = paletteState.head
                column.glyphs[row] = randomGlyphIndex()

                ctx.fillStyle = rgba(paletteState.bright, 0.12)
                ctx.fillRect(x, y, cellWidth - 1, cellHeight - 1)
            }

            if (glitch && Math.random() < 0.012 + glitchPulse * 0.02) {
                const jitter = Math.random() < 0.5 ? -1 : 1
                ctx.fillStyle = rgba(paletteState.glitch, Math.min(1, alpha * 0.65))
                ctx.fillText(glyph, x + jitter, y)
                color = mixRgb(color, paletteState.glitch, 0.55)
                alpha = Math.min(1, alpha + 0.12)
            }

            ctx.fillStyle = rgba(color, alpha)
            ctx.fillText(glyph, x, y)
        }
    }

    return (ctx, time, c) => {
        const speed = c.speed as number
        const density = c.density as number
        const trailLength = c.trailLength as number
        const charSize = c.charSize as number
        const colorMode = c.colorMode as string
        const glitch = c.glitch as boolean
        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const dt = lastTime < 0 ? 1 / 60 : Math.min(0.08, time - lastTime)
        lastTime = time

        // Detect control changes
        if (colorMode !== prevColorMode) {
            paletteState = resolvePalette(colorMode)
            prevColorMode = colorMode
        }
        if (charSize !== prevCharSize) {
            needsGridSync = true
            prevCharSize = charSize
        }

        syncGrid(w, h, charSize, density)
        updateColumns(dt, speed, density, trailLength)

        // Clear with palette background
        ctx.fillStyle = paletteState.background
        ctx.fillRect(0, 0, w, h)

        drawAtmosphere(ctx, w, h, trailLength, density, glitch)

        // Draw streams
        const fontSize = Math.max(8, Math.round(cellHeight * 0.9))
        const glitchPulse = glitch ? 0.5 + 0.5 * Math.sin(time * 18) : 0

        ctx.save()
        ctx.imageSmoothingEnabled = false
        ctx.textBaseline = 'top'
        ctx.textAlign = 'left'
        ctx.font = `700 ${fontSize}px "JetBrains Mono", "Fira Code", "SF Mono", Consolas, monospace`

        for (let columnIndex = 0; columnIndex < columns.length; columnIndex++) {
            const column = columns[columnIndex]
            if (!column.active) {
                drawDormantGlyph(ctx, columnIndex, density)
                continue
            }
            drawColumnTrail(ctx, column, columnIndex, glitchPulse, trailLength, density, glitch)
        }

        ctx.restore()
    }
}, {
    description: 'Community-style matrix rain with crisp block glyph columns and configurable palettes',
})
