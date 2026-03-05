import 'reflect-metadata'
import {
    BooleanControl,
    CanvasEffect,
    ComboboxControl,
    Effect,
    NumberControl,
    getControlValue,
    initializeEffect,
    normalizeSpeed,
} from '@hypercolor/sdk'

interface DigitalRainControls {
    speed: number
    density: number
    trailLength: number
    charSize: number
    palette: string
    glitch: boolean
}

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
    head: number
    speedBias: number
    trailBias: number
    active: boolean
    respawnGap: number
    glyphs: number[]
    mutateClock: number
    mutateEvery: number
}

const COLOR_MODES = ['Matrix', 'Phosphor', 'SilkCircuit', 'Cyberpunk', 'Ice']

const GLYPHS = [
    '█',
    '▓',
    '▒',
    '░',
    '▉',
    '▊',
    '▋',
    '▌',
    '▐',
    '▀',
    '▄',
    '■',
    '▣',
    '▤',
    '▥',
    '▦',
    '▧',
    '▨',
    '▩',
    '╬',
    '╫',
    '╪',
    '╳',
    '╱',
    '╲',
    'ｱ',
    'ｲ',
    'ｷ',
    'ｼ',
    'ﾂ',
    'ﾑ',
    'ﾓ',
    '0',
    '1',
]

const PALETTES: Record<string, RainPalette> = {
    Matrix: {
        background: '#020903',
        shadow: { r: 5, g: 28, b: 10 },
        trail: { r: 42, g: 138, b: 56 },
        bright: { r: 138, g: 248, b: 162 },
        head: { r: 236, g: 255, b: 238 },
        glitch: { r: 182, g: 255, b: 196 },
    },
    Phosphor: {
        background: '#120903',
        shadow: { r: 32, g: 18, b: 4 },
        trail: { r: 156, g: 102, b: 32 },
        bright: { r: 250, g: 185, b: 88 },
        head: { r: 255, g: 245, b: 220 },
        glitch: { r: 255, g: 212, b: 120 },
    },
    SilkCircuit: {
        background: '#06060f',
        shadow: { r: 20, g: 12, b: 40 },
        trail: { r: 128, g: 255, b: 234 },
        bright: { r: 225, g: 53, b: 255 },
        head: { r: 242, g: 255, b: 252 },
        glitch: { r: 255, g: 106, b: 193 },
    },
    Cyberpunk: {
        background: '#08020f',
        shadow: { r: 22, g: 8, b: 32 },
        trail: { r: 255, g: 106, b: 193 },
        bright: { r: 132, g: 245, b: 255 },
        head: { r: 250, g: 240, b: 255 },
        glitch: { r: 255, g: 235, b: 88 },
    },
    Ice: {
        background: '#010917',
        shadow: { r: 6, g: 22, b: 40 },
        trail: { r: 62, g: 160, b: 232 },
        bright: { r: 146, g: 228, b: 255 },
        head: { r: 236, g: 252, b: 255 },
        glitch: { r: 162, g: 252, b: 255 },
    },
}

@Effect({
    name: 'Digital Rain',
    description: 'Community-style matrix rain with crisp block glyph columns and configurable palettes',
    author: 'Hypercolor',
    audioReactive: false,
})
class DigitalRain extends CanvasEffect<DigitalRainControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Column fall speed' })
    speed!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 62, tooltip: 'Active column density' })
    density!: number

    @NumberControl({ label: 'Trail', min: 5, max: 100, default: 58, tooltip: 'Trail length behind each head' })
    trailLength!: number

    @NumberControl({ label: 'Size', min: 0, max: 100, default: 42, tooltip: 'Character grid size' })
    charSize!: number

    @ComboboxControl({ label: 'Color Mode', values: COLOR_MODES, default: 'Matrix', tooltip: 'Palette preset' })
    palette!: string

    @BooleanControl({ label: 'Glitch', default: false, tooltip: 'Adds occasional data tears and flicker' })
    glitch!: boolean

    private controlState: DigitalRainControls = {
        speed: normalizeSpeed(5),
        density: 62,
        trailLength: 58,
        charSize: 42,
        palette: 'Matrix',
        glitch: false,
    }

    private paletteState: RainPalette = PALETTES.Matrix
    private columns: ColumnState[] = []
    private rows = 0
    private cols = 0
    private cellWidth = 10
    private cellHeight = 16
    private lastCanvasWidth = 0
    private lastCanvasHeight = 0
    private needsGridSync = true

    constructor() {
        super({ id: 'digital-rain', name: 'Digital Rain', backgroundColor: PALETTES.Matrix.background })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.density = getControlValue('density', 62)
        this.trailLength = getControlValue('trailLength', 58)
        this.charSize = getControlValue('charSize', 42)
        this.palette = getControlValue('palette', 'Matrix')
        this.glitch = getControlValue('glitch', false)

        this.controlState = this.getControlValues()
        this.paletteState = this.resolvePalette(this.controlState.palette)
        this.backgroundColor = this.paletteState.background
        this.needsGridSync = true
    }

    protected getControlValues(): DigitalRainControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            density: getControlValue('density', 62),
            trailLength: getControlValue('trailLength', 58),
            charSize: getControlValue('charSize', 42),
            palette: getControlValue('palette', 'Matrix'),
            glitch: getControlValue('glitch', false),
        }
    }

    protected async loadResources(): Promise<void> {
        this.paletteState = this.resolvePalette(this.controlState.palette)
        this.backgroundColor = this.paletteState.background
        if (this.ctx) this.ctx.imageSmoothingEnabled = false
        this.needsGridSync = true
    }

    protected applyControls(controls: DigitalRainControls): void {
        const sizeChanged = controls.charSize !== this.controlState.charSize
        const paletteChanged = controls.palette !== this.controlState.palette

        this.controlState = controls

        if (paletteChanged) {
            this.paletteState = this.resolvePalette(controls.palette)
            this.backgroundColor = this.paletteState.background
        }

        if (sizeChanged) this.needsGridSync = true
    }

    protected draw(time: number, deltaTime: number): void {
        if (!this.ctx || !this.canvas) return

        const ctx = this.ctx
        const w = this.canvas.width
        const h = this.canvas.height
        const dt = deltaTime > 0 ? Math.min(0.08, deltaTime) : 1 / 60

        this.syncGrid(w, h)
        this.updateColumns(dt)
        this.drawAtmosphere(ctx, w, h)
        this.drawStreams(ctx, time)
    }

    private syncGrid(w: number, h: number): void {
        const nextCellWidth = Math.max(6, Math.round(6 + this.controlState.charSize * 0.14))
        const nextCellHeight = Math.max(nextCellWidth + 2, Math.round(nextCellWidth * 1.55))
        const nextCols = Math.max(8, Math.floor(w / nextCellWidth))
        const nextRows = Math.max(8, Math.floor(h / nextCellHeight))

        const canvasChanged = this.lastCanvasWidth !== w || this.lastCanvasHeight !== h
        const gridChanged =
            this.cellWidth !== nextCellWidth ||
            this.cellHeight !== nextCellHeight ||
            this.cols !== nextCols ||
            this.rows !== nextRows

        if (!this.needsGridSync && !canvasChanged && !gridChanged) return

        this.lastCanvasWidth = w
        this.lastCanvasHeight = h
        this.cellWidth = nextCellWidth
        this.cellHeight = nextCellHeight
        this.cols = nextCols
        this.rows = nextRows
        this.rebuildColumns()
        this.needsGridSync = false
    }

    private rebuildColumns(): void {
        this.columns = []
        for (let i = 0; i < this.cols; i++) {
            this.columns.push(this.createColumn(true))
        }
    }

    private createColumn(seedHead: boolean): ColumnState {
        const glyphs = new Array<number>(this.rows)
        for (let i = 0; i < this.rows; i++) glyphs[i] = this.randomGlyphIndex()

        return {
            head: seedHead ? this.randomRange(-this.rows, this.rows) : -this.randomRange(2, this.rows * 1.2 + 2),
            speedBias: this.randomRange(0.62, 1.52),
            trailBias: this.randomRange(0.72, 1.28),
            active: Math.random() < this.activeProbability(),
            respawnGap: this.randomRange(2, 8),
            glyphs,
            mutateClock: Math.random() * 0.2,
            mutateEvery: this.randomRange(0.03, 0.16),
        }
    }

    private updateColumns(dt: number): void {
        const rowsPerSecond = 3.5 + this.controlState.speed * 8.2
        const wakeChance = dt * (0.14 + this.controlState.density * 0.004)

        for (const column of this.columns) {
            if (!column.active) {
                if (Math.random() < wakeChance * (0.35 + this.activeProbability())) {
                    this.resetColumn(column, true)
                }
                continue
            }

            column.head += rowsPerSecond * column.speedBias * dt

            const trailCells = this.columnTrailCells(column)
            if (column.head - trailCells > this.rows + column.respawnGap) {
                this.resetColumn(column, false)
                continue
            }

            column.mutateClock += dt
            if (column.mutateClock >= column.mutateEvery) {
                column.mutateClock = 0
                this.mutateColumnGlyphs(column)
            }
        }
    }

    private resetColumn(column: ColumnState, forceActive: boolean): void {
        column.active = forceActive || Math.random() < this.activeProbability()
        column.head = -this.randomRange(2, this.rows * (0.4 + Math.random() * 0.9))
        column.speedBias = this.randomRange(0.6, 1.58)
        column.trailBias = this.randomRange(0.7, 1.35)
        column.respawnGap = this.randomRange(2, 9)
        column.mutateClock = Math.random() * 0.25
        column.mutateEvery = this.randomRange(0.03, 0.18)

        if (column.glyphs.length !== this.rows) {
            column.glyphs = new Array<number>(this.rows)
            for (let i = 0; i < this.rows; i++) column.glyphs[i] = this.randomGlyphIndex()
            return
        }

        for (let i = 0; i < this.rows; i++) {
            if (Math.random() < 0.34) {
                column.glyphs[i] = this.randomGlyphIndex()
            }
        }
    }

    private mutateColumnGlyphs(column: ColumnState): void {
        const mutationCount = 1 + (Math.random() < 0.4 ? 1 : 0)
        const headRow = Math.floor(column.head)

        for (let i = 0; i < mutationCount; i++) {
            const offset = Math.floor(Math.random() * this.columnTrailCells(column))
            const jitter = Math.floor((Math.random() - 0.5) * 6)
            const row = this.wrapRow(headRow - offset + jitter)
            column.glyphs[row] = this.randomGlyphIndex()
        }

        if (Math.random() < 0.12) {
            const randomRow = Math.floor(Math.random() * this.rows)
            column.glyphs[randomRow] = this.randomGlyphIndex()
        }
    }

    private drawAtmosphere(ctx: CanvasRenderingContext2D, w: number, h: number): void {
        const haze = ctx.createLinearGradient(0, 0, 0, h)
        haze.addColorStop(0, this.rgba(this.paletteState.shadow, 0.28))
        haze.addColorStop(0.58, this.rgba(this.paletteState.shadow, 0.1))
        haze.addColorStop(1, this.rgba(this.paletteState.shadow, 0.02))
        ctx.fillStyle = haze
        ctx.fillRect(0, 0, w, h)

        const scanlineAlpha = 0.02 + (this.controlState.trailLength / 100) * 0.05
        ctx.fillStyle = this.rgba(this.paletteState.trail, scanlineAlpha)
        for (let y = 1; y < h; y += 3) {
            ctx.fillRect(0, y, w, 1)
        }

        if (!this.controlState.glitch || Math.random() >= 0.14) return

        const bandY = Math.floor(Math.random() * h)
        const bandHeight = 1 + Math.floor(Math.random() * 3)
        ctx.fillStyle = this.rgba(this.paletteState.glitch, 0.08 + Math.random() * 0.2)
        ctx.fillRect(0, bandY, w, bandHeight)
    }

    private drawStreams(ctx: CanvasRenderingContext2D, time: number): void {
        const fontSize = Math.max(8, Math.round(this.cellHeight * 0.9))
        const glitchPulse = this.controlState.glitch ? 0.5 + 0.5 * Math.sin(time * 18) : 0

        ctx.save()
        ctx.imageSmoothingEnabled = false
        ctx.textBaseline = 'top'
        ctx.textAlign = 'left'
        ctx.font = `700 ${fontSize}px "JetBrains Mono", "Fira Code", "SF Mono", Consolas, monospace`

        for (let columnIndex = 0; columnIndex < this.columns.length; columnIndex++) {
            const column = this.columns[columnIndex]
            if (!column.active) {
                this.drawDormantGlyph(ctx, columnIndex)
                continue
            }
            this.drawColumnTrail(ctx, column, columnIndex, glitchPulse)
        }

        ctx.restore()
    }

    private drawDormantGlyph(ctx: CanvasRenderingContext2D, columnIndex: number): void {
        if (Math.random() >= 0.01 + (this.controlState.density / 100) * 0.02) return

        const row = Math.floor(Math.random() * this.rows)
        const glyph = GLYPHS[this.randomGlyphIndex()]
        const x = columnIndex * this.cellWidth
        const y = row * this.cellHeight
        ctx.fillStyle = this.rgba(this.paletteState.trail, 0.05)
        ctx.fillText(glyph, x, y)
    }

    private drawColumnTrail(
        ctx: CanvasRenderingContext2D,
        column: ColumnState,
        columnIndex: number,
        glitchPulse: number,
    ): void {
        const trailCells = this.columnTrailCells(column)
        const headRow = Math.floor(column.head)
        const x = columnIndex * this.cellWidth

        for (let step = 0; step < trailCells; step++) {
            const row = headRow - step
            if (row < 0 || row >= this.rows) continue

            const energy = 1 - step / trailCells
            const fade = Math.pow(energy, 1.6)
            if (fade < 0.02) continue

            const glyph = GLYPHS[column.glyphs[row] ?? 0]
            const y = row * this.cellHeight
            let color = this.mixRgb(this.paletteState.trail, this.paletteState.bright, Math.pow(energy, 0.58))
            let alpha = (0.12 + this.controlState.trailLength / 135) * fade

            if (step === 0) {
                alpha = 0.98
                color = this.paletteState.head
                column.glyphs[row] = this.randomGlyphIndex()

                ctx.fillStyle = this.rgba(this.paletteState.bright, 0.18)
                ctx.fillRect(x, y, this.cellWidth - 1, this.cellHeight - 1)
            }

            if (this.controlState.glitch && Math.random() < 0.012 + glitchPulse * 0.02) {
                const jitter = Math.random() < 0.5 ? -1 : 1
                ctx.fillStyle = this.rgba(this.paletteState.glitch, Math.min(1, alpha * 0.65))
                ctx.fillText(glyph, x + jitter, y)
                color = this.mixRgb(color, this.paletteState.glitch, 0.55)
                alpha = Math.min(1, alpha + 0.12)
            }

            ctx.fillStyle = this.rgba(color, alpha)
            ctx.fillText(glyph, x, y)
        }
    }

    private columnTrailCells(column: ColumnState): number {
        const baseTrail = 4 + this.controlState.trailLength * 0.26
        const densityBoost = 0.86 + this.activeProbability() * 0.24
        return Math.max(4, Math.round(baseTrail * column.trailBias * densityBoost))
    }

    private activeProbability(): number {
        const normalized = this.clamp((this.controlState.density - 10) / 90, 0, 1)
        return 0.14 + normalized * 0.84
    }

    private resolvePalette(mode: string): RainPalette {
        return PALETTES[mode] ?? PALETTES.Matrix
    }

    private wrapRow(row: number): number {
        if (this.rows <= 0) return 0
        const wrapped = row % this.rows
        return wrapped < 0 ? wrapped + this.rows : wrapped
    }

    private mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
        const blend = this.clamp(t, 0, 1)
        return {
            r: Math.round(a.r + (b.r - a.r) * blend),
            g: Math.round(a.g + (b.g - a.g) * blend),
            b: Math.round(a.b + (b.b - a.b) * blend),
        }
    }

    private rgba(color: Rgb, alpha: number): string {
        return `rgba(${color.r}, ${color.g}, ${color.b}, ${this.clamp(alpha, 0, 1).toFixed(3)})`
    }

    private randomGlyphIndex(): number {
        return Math.floor(Math.random() * GLYPHS.length)
    }

    private randomRange(min: number, max: number): number {
        return min + Math.random() * (max - min)
    }

    private clamp(value: number, min: number, max: number): number {
        if (value < min) return min
        if (value > max) return max
        return value
    }
}

const effect = new DigitalRain()
initializeEffect(() => effect.initialize(), { instance: effect })
