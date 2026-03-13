import { canvas, color, combo } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface Rgb {
    r: number
    g: number
    b: number
}

interface RainPalette {
    background: string
    shadow: Rgb
    trail: Rgb
    bright: Rgb
    head: Rgb
    glitch: Rgb
}

interface ColumnState {
    active: boolean
    glyphs: number[]
    head: number
    mutateClock: number
    mutateEvery: number
    respawnGap: number
    speedBias: number
    stepAccumulator: number
    trailBias: number
}

// ── Constants ────────────────────────────────────────────────────────────

const COLOR_MODES = ['Custom', 'Cyberpunk', 'Ice', 'Matrix', 'Phosphor', 'SilkCircuit'] as const

const GLYPHS = [':', '.', '/', '\\', '+', '=', '|', '0', '1', '2', '4', '7', '9', 'A', 'C', 'E', 'K', 'N', 'R', 'X', 'Z', 'ｱ', 'ｳ', 'ｴ', 'ｵ', 'ｶ', 'ｷ', 'ｹ', 'ｺ', 'ｻ', 'ｼ', 'ｽ', 'ﾀ', 'ﾂ', 'ﾅ', 'ﾊ', 'ﾏ', 'ﾐ', 'ﾑ', 'ﾒ', 'ﾓ', 'ﾔ', 'ﾕ', 'ﾗ', 'ﾜ']

const PALETTES: Record<string, RainPalette> = {
    Cyberpunk: {
        background: '#05010c',
        bright: { b: 248, g: 255, r: 116 },
        glitch: { b: 72, g: 154, r: 255 },
        head: { b: 234, g: 255, r: 150 },
        shadow: { b: 20, g: 5, r: 14 },
        trail: { b: 193, g: 106, r: 255 },
    },
    Ice: {
        background: '#010611',
        bright: { b: 255, g: 228, r: 98 },
        glitch: { b: 255, g: 255, r: 118 },
        head: { b: 255, g: 248, r: 126 },
        shadow: { b: 26, g: 14, r: 5 },
        trail: { b: 255, g: 168, r: 34 },
    },
    Matrix: {
        background: '#010401',
        bright: { b: 122, g: 255, r: 74 },
        glitch: { b: 120, g: 255, r: 174 },
        head: { b: 154, g: 255, r: 126 },
        shadow: { b: 5, g: 12, r: 4 },
        trail: { b: 52, g: 170, r: 18 },
    },
    Phosphor: {
        background: '#090302',
        bright: { b: 42, g: 144, r: 255 },
        glitch: { b: 18, g: 112, r: 255 },
        head: { b: 74, g: 176, r: 255 },
        shadow: { b: 3, g: 8, r: 18 },
        trail: { b: 18, g: 88, r: 194 },
    },
    SilkCircuit: {
        background: '#02030a',
        bright: { b: 255, g: 53, r: 225 },
        glitch: { b: 193, g: 106, r: 255 },
        head: { b: 238, g: 255, r: 112 },
        shadow: { b: 24, g: 8, r: 10 },
        trail: { b: 234, g: 255, r: 128 },
    },
}

const WHITE: Rgb = { b: 255, g: 255, r: 255 }

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

function hexToRgb(hex: string): Rgb {
    const normalized = hex.trim().replace('#', '')
    const expanded =
        normalized.length === 3
            ? normalized
                  .split('')
                  .map((char) => `${char}${char}`)
                  .join('')
            : normalized

    if (!/^[0-9a-fA-F]{6}$/.test(expanded)) {
        return WHITE
    }

    const value = Number.parseInt(expanded, 16)
    return {
        b: value & 255,
        g: (value >> 8) & 255,
        r: (value >> 16) & 255,
    }
}

function rgba(color: Rgb, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const blend = clamp(t, 0, 1)
    return {
        b: Math.round(a.b + (b.b - a.b) * blend),
        g: Math.round(a.g + (b.g - a.g) * blend),
        r: Math.round(a.r + (b.r - a.r) * blend),
    }
}

function resolvePresetPalette(mode: string): RainPalette {
    return PALETTES[mode] ?? PALETTES.Matrix
}

function resolvePalette(controls: Record<string, unknown>): RainPalette {
    const mode = controls.colorMode as string
    if (mode !== 'Custom') {
        return resolvePresetPalette(mode)
    }

    const background = controls.bgColor as string
    const trail = hexToRgb(controls.rainColor as string)
    const head = hexToRgb(controls.headColor as string)
    const shadow = mixRgb(hexToRgb(background), trail, 0.12)
    const bright = mixRgb(trail, head, 0.42)
    const glitch = mixRgb(trail, head, 0.68)

    return {
        background,
        bright,
        glitch,
        head,
        shadow,
        trail,
    }
}

function activeProbability(density: number): number {
    const normalized = clamp((density - 10) / 90, 0, 1)
    return 0.14 + normalized * 0.84
}

function columnTrailCells(column: ColumnState, trailLength: number, density: number): number {
    const baseTrail = 3 + trailLength * 0.16
    const densityBoost = 0.9 + activeProbability(density) * 0.14
    return Math.max(3, Math.round(baseTrail * column.trailBias * densityBoost))
}

function createColumn(rows: number, seedHead: boolean, density: number): ColumnState {
    const glyphs = new Array<number>(rows)
    for (let i = 0; i < rows; i++) glyphs[i] = randomGlyphIndex()

    const head = seedHead ? Math.floor(randomRange(-rows, rows)) : -Math.floor(randomRange(2, rows * 1.2 + 2))
    return {
        active: Math.random() < activeProbability(density),
        glyphs,
        head,
        mutateClock: Math.random() * 0.2,
        mutateEvery: randomRange(0.18, 0.52),
        respawnGap: randomRange(2, 8),
        speedBias: randomRange(0.74, 1.34),
        stepAccumulator: Math.random(),
        trailBias: randomRange(0.82, 1.22),
    }
}

function resetColumn(column: ColumnState, rows: number, density: number, forceActive: boolean): void {
    column.active = forceActive || Math.random() < activeProbability(density)
    column.head = -Math.floor(randomRange(2, rows * (0.4 + Math.random() * 0.9)))
    column.speedBias = randomRange(0.72, 1.38)
    column.trailBias = randomRange(0.8, 1.24)
    column.respawnGap = randomRange(2, 9)
    column.mutateClock = Math.random() * 0.25
    column.mutateEvery = randomRange(0.18, 0.58)
    column.stepAccumulator = Math.random()

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
    const visibleCells = columnTrailCells(column, trailLength, density)
    const mutationCount = 1 + (Math.random() < 0.1 ? 1 : 0)

    for (let i = 0; i < mutationCount; i++) {
        const offset = 1 + Math.floor(Math.random() * Math.max(1, visibleCells - 1))
        const row = column.head - offset
        if (row >= 0 && row < rows) {
            column.glyphs[row] = randomGlyphIndex()
        }
    }
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful(
    'Digital Rain',
    {
        colorMode: combo('Color Mode', [...COLOR_MODES], { default: 'Matrix' }),
        bgColor: color('Custom Background', '#010401'),
        rainColor: color('Custom Rain', '#12aa34'),
        headColor: color('Custom Head', '#7eff9a'),
        speed: [1, 10, 5],
        density: [10, 100, 62],
        trailLength: [5, 100, 58],
        charSize: [0, 100, 54],
        leadWhite: [0, 100, 14],
        glitch: false,
    },
    () => {
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

        function rebuildColumns(density: number): void {
            columns = []
            for (let i = 0; i < cols; i++) {
                columns.push(createColumn(rows, true, density))
            }
        }

        function syncGrid(w: number, h: number, charSize: number, density: number): void {
            const nextCellWidth = Math.max(5, Math.round(5 + charSize * 0.09))
            const nextCellHeight = Math.max(nextCellWidth + 4, Math.round(nextCellWidth * 1.62))
            const nextCols = Math.max(8, Math.floor(w / nextCellWidth))
            const nextRows = Math.max(8, Math.floor(h / nextCellHeight))

            const canvasChanged = lastCanvasWidth !== w || lastCanvasHeight !== h
            const gridChanged =
                cellWidth !== nextCellWidth || cellHeight !== nextCellHeight || cols !== nextCols || rows !== nextRows

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
            const rowsPerSecond = 1.4 + speed * 2.35
            const wakeChance = dt * (0.09 + density * 0.0026)

            for (const column of columns) {
                if (!column.active) {
                    if (Math.random() < wakeChance * (0.35 + activeProbability(density))) {
                        resetColumn(column, rows, density, true)
                    }
                    continue
                }

                column.stepAccumulator += rowsPerSecond * column.speedBias * dt
                const steps = Math.floor(column.stepAccumulator)
                if (steps > 0) {
                    column.stepAccumulator -= steps

                    for (let step = 0; step < steps; step++) {
                        column.head += 1
                        if (column.head >= 0 && column.head < rows) {
                            column.glyphs[column.head] = randomGlyphIndex()
                        }
                    }
                }

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

        function drawBackdrop(ctx: CanvasRenderingContext2D, w: number, h: number, glitch: boolean): void {
            const vignette = ctx.createLinearGradient(0, 0, 0, h)
            vignette.addColorStop(0, rgba(paletteState.shadow, 0.02))
            vignette.addColorStop(1, rgba(paletteState.shadow, 0.08))
            ctx.fillStyle = vignette
            ctx.fillRect(0, 0, w, h)

            if (!glitch || Math.random() >= 0.04) return

            const bandY = Math.floor(Math.random() * h)
            const bandHeight = 1 + Math.floor(Math.random() * 3)
            ctx.fillStyle = rgba(paletteState.glitch, 0.08 + Math.random() * 0.08)
            ctx.fillRect(0, bandY, w, bandHeight)
        }

        function drawColumnTrail(
            ctx: CanvasRenderingContext2D,
            column: ColumnState,
            columnIndex: number,
            trailLength: number,
            density: number,
            leadWhite: number,
            glitch: boolean,
        ): void {
            const trailCells = columnTrailCells(column, trailLength, density)
            const x = columnIndex * cellWidth

            for (let step = 0; step < trailCells; step++) {
                const rowIndex = column.head - step
                if (rowIndex < 0 || rowIndex >= rows) continue

                const energy = 1 - step / trailCells
                if (energy < 0.08) continue
                if (step > 4 && energy < 0.34 && step % 2 === 1) continue

                const glyph = GLYPHS[column.glyphs[rowIndex] ?? 0]
                const y = rowIndex * cellHeight
                let color = paletteState.shadow
                let alpha = 0.26

                if (step === 0) {
                    const leadMix = clamp(leadWhite / 100, 0, 1)
                    alpha = 0.98
                    color = mixRgb(paletteState.head, WHITE, leadMix * 0.9)
                } else if (energy > 0.72) {
                    color = paletteState.bright
                    alpha = 0.9
                } else if (energy > 0.46) {
                    color = mixRgb(paletteState.trail, paletteState.bright, 0.3)
                    alpha = 0.72
                } else if (energy > 0.24) {
                    color = paletteState.trail
                    alpha = 0.5
                } else {
                    color = paletteState.shadow
                    alpha = 0.3
                }

                if (glitch && step < 2 && Math.random() < 0.018) {
                    const jitter = Math.random() < 0.5 ? -1 : 1
                    ctx.fillStyle = rgba(paletteState.glitch, Math.min(1, alpha * 0.65))
                    ctx.fillText(glyph, x + jitter, y)
                    color = mixRgb(color, paletteState.glitch, 0.42)
                    alpha = Math.min(1, alpha + 0.08)
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
            const leadWhite = c.leadWhite as number
            const glitch = c.glitch as boolean
            const w = ctx.canvas.width
            const h = ctx.canvas.height
            const dt = lastTime < 0 ? 1 / 60 : Math.min(0.08, time - lastTime)
            lastTime = time

            paletteState = resolvePalette(c as Record<string, unknown>)
            if (charSize !== prevCharSize) {
                needsGridSync = true
                prevCharSize = charSize
            }

            syncGrid(w, h, charSize, density)
            updateColumns(dt, speed, density, trailLength)

            // Clear with palette background
            ctx.fillStyle = paletteState.background
            ctx.fillRect(0, 0, w, h)

            drawBackdrop(ctx, w, h, glitch)

            // Draw streams
            const fontSize = Math.max(8, Math.round(cellHeight * 0.8))

            ctx.save()
            ctx.imageSmoothingEnabled = false
            ctx.textBaseline = 'top'
            ctx.textAlign = 'left'
            ctx.font = `700 ${fontSize}px "JetBrains Mono", "Fira Code", "SF Mono", Consolas, monospace`

            for (let columnIndex = 0; columnIndex < columns.length; columnIndex++) {
                const column = columns[columnIndex]
                if (!column.active) continue
                drawColumnTrail(ctx, column, columnIndex, trailLength, density, leadWhite, glitch)
            }

            ctx.restore()
        }
    },
    {
        description:
            'Terminal-crisp digital rain with discrete stepped streaks, hard tail shading, and configurable white stream heads',
        presets: [
            {
                name: 'Mainframe Breach',
                description: 'A classified system compromised at 3 AM — frantic green columns racing down a black void, every glyph a stolen secret',
                controls: {
                    colorMode: 'Matrix',
                    bgColor: '#010401',
                    rainColor: '#12aa34',
                    headColor: '#7eff9a',
                    speed: 8,
                    density: 88,
                    trailLength: 38,
                    charSize: 34,
                    leadWhite: 72,
                    glitch: true,
                },
            },
            {
                name: 'Abandoned Server Room',
                description: 'Dust on the racks, one last terminal still scrolling — amber phosphor trails fading into a warm brown-black silence',
                controls: {
                    colorMode: 'Phosphor',
                    bgColor: '#090302',
                    rainColor: '#c25812',
                    headColor: '#ffb04a',
                    speed: 2,
                    density: 28,
                    trailLength: 85,
                    charSize: 72,
                    leadWhite: 5,
                    glitch: false,
                },
            },
            {
                name: 'Shinjuku After Rain',
                description: 'Neon kanji reflected in wet asphalt — magenta and cyan streams bleeding through a purple haze at 2 AM',
                controls: {
                    colorMode: 'Cyberpunk',
                    bgColor: '#05010c',
                    rainColor: '#ff6ac1',
                    headColor: '#80ffea',
                    speed: 5,
                    density: 70,
                    trailLength: 62,
                    charSize: 48,
                    leadWhite: 30,
                    glitch: true,
                },
            },
            {
                name: 'Cryo Vault',
                description: 'Frozen data streams in a subterranean archive — glacial blue columns descending in slow crystalline silence',
                controls: {
                    colorMode: 'Ice',
                    bgColor: '#010611',
                    rainColor: '#22a8ff',
                    headColor: '#7ef8ff',
                    speed: 3,
                    density: 45,
                    trailLength: 95,
                    charSize: 60,
                    leadWhite: 48,
                    glitch: false,
                },
            },
            {
                name: 'Ghost in the Wire',
                description: 'A rogue AI splintering across the network — SilkCircuit violet pulses tearing through corrupted data at impossible speed',
                controls: {
                    colorMode: 'SilkCircuit',
                    bgColor: '#02030a',
                    rainColor: '#e135ff',
                    headColor: '#80ffea',
                    speed: 10,
                    density: 100,
                    trailLength: 22,
                    charSize: 26,
                    leadWhite: 90,
                    glitch: true,
                },
            },
        ],
    },
)
