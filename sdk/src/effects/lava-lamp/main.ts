import 'reflect-metadata'
import {
    BooleanControl,
    CanvasEffect,
    ComboboxControl,
    ColorControl,
    Effect,
    NumberControl,
    getControlValue,
    initializeEffect,
} from '@hypercolor/sdk'

interface LavaLampControls {
    bgColor: string
    bgCycle: boolean
    theme: string
    color1: string
    color2: string
    color3: string
    rainbow: boolean
    speed: number
    cycleSpeed: number
    bCount: number
}

interface Blob {
    x: number
    y: number
    vx: number
    vy: number
    radius: number
    phase: number
    seed: number
}

interface Rgb {
    r: number
    g: number
    b: number
}

interface ThemePalette {
    color1: string
    color2: string
    color3: string
}

const THEMES = ['Custom', 'Bubblegum', 'Lagoon', 'Toxic', 'Aurora', 'Molten', 'Synthwave', 'Citrus']

const THEME_PALETTES: Record<string, ThemePalette> = {
    Custom:    { color1: '#000ded', color2: '#e40020', color3: '#ffcf4d' },
    Bubblegum: { color1: '#ff4f9a', color2: '#ff96c1', color3: '#ffd4ef' },
    Lagoon:    { color1: '#3cf2df', color2: '#4a96ff', color3: '#163dff' },
    Toxic:     { color1: '#98ff4a', color2: '#0ae0cb', color3: '#6c2bff' },
    Aurora:    { color1: '#33f587', color2: '#3fdcff', color3: '#8c4bff' },
    Molten:    { color1: '#ff6329', color2: '#ffb021', color3: '#ffe66b' },
    Synthwave: { color1: '#ff4ed6', color2: '#8f48ff', color3: '#42d9ff' },
    Citrus:    { color1: '#ffd84d', color2: '#ff9343', color3: '#ff5778' },
}

@Effect({
    name: 'Lava Lamp',
    description: 'Community-inspired contour metaballs with crisp RGB blends and merge/split motion',
    author: 'Hypercolor',
    audioReactive: false,
})
class LavaLamp extends CanvasEffect<LavaLampControls> {
    @ColorControl({ label: 'Background Color', default: '#0b0312', tooltip: 'Backdrop color' })
    bgColor!: string

    @BooleanControl({ label: 'Background Color Cycle', default: false, tooltip: 'Cycle background hue over time' })
    bgCycle!: boolean

    @ComboboxControl({ label: 'Theme', values: THEMES, default: 'Custom', tooltip: 'Curated lava color palettes' })
    theme!: string

    @ColorControl({ label: 'Color 1', default: '#000ded', tooltip: 'Primary lava color' })
    color1!: string

    @ColorControl({ label: 'Color 2', default: '#e40020', tooltip: 'Secondary lava color' })
    color2!: string

    @ColorControl({ label: 'Color 3', default: '#ffcf4d', tooltip: 'Highlight lava color' })
    color3!: string

    @BooleanControl({ label: 'Lava Color Cycle', default: false, tooltip: 'Cycle lava hues continuously' })
    rainbow!: boolean

    @NumberControl({ label: 'Speed', min: 1, max: 100, default: 22, tooltip: 'Blob movement speed' })
    speed!: number

    @NumberControl({ label: 'Color Speed', min: 1, max: 100, default: 22, tooltip: 'Hue cycling speed' })
    cycleSpeed!: number

    @NumberControl({ label: 'Number of Blobs', min: 1, max: 18, default: 6, tooltip: 'How many metaballs are active' })
    bCount!: number

    private controls: LavaLampControls = {
        bgColor: '#0b0312',
        bgCycle: false,
        theme: 'Custom',
        color1: '#000ded',
        color2: '#e40020',
        color3: '#ffcf4d',
        rainbow: false,
        speed: 22,
        cycleSpeed: 22,
        bCount: 6,
    }

    private blobs: Blob[] = []
    private blobCount = 0
    private bgHue = 0
    private lavaHue = 0

    private field = new Float32Array(0)
    private cols = 0
    private rows = 0

    private readonly step = 1
    private readonly threshold = 0.94
    private readonly contourLevels = [1.00, 1.34, 1.78]

    constructor() {
        super({ id: 'lava-lamp', name: 'Lava Lamp', backgroundColor: '#0b0312' })
    }

    protected initializeControls(): void {
        this.bgColor = getControlValue('bgColor', '#0b0312')
        this.bgCycle = getControlValue('bgCycle', false)
        this.theme = getControlValue('theme', 'Custom')
        this.color1 = getControlValue('color1', '#000ded')
        this.color2 = getControlValue('color2', '#e40020')
        this.color3 = getControlValue('color3', '#ffcf4d')
        this.rainbow = getControlValue('rainbow', false)
        this.speed = getControlValue('speed', 22)
        this.cycleSpeed = getControlValue('cycleSpeed', 22)
        this.bCount = getControlValue('bCount', 6)
    }

    protected getControlValues(): LavaLampControls {
        return {
            bgColor: this.normalizeHexColor(getControlValue('bgColor', this.controls.bgColor), this.controls.bgColor),
            bgCycle: this.coerceBoolean(getControlValue('bgCycle', this.controls.bgCycle), this.controls.bgCycle),
            theme: typeof getControlValue('theme', this.controls.theme) === 'string'
                ? getControlValue('theme', this.controls.theme)
                : this.controls.theme,
            color1: this.normalizeHexColor(getControlValue('color1', this.controls.color1), this.controls.color1),
            color2: this.normalizeHexColor(getControlValue('color2', this.controls.color2), this.controls.color2),
            color3: this.normalizeHexColor(getControlValue('color3', this.controls.color3), this.controls.color3),
            rainbow: this.coerceBoolean(getControlValue('rainbow', this.controls.rainbow), this.controls.rainbow),
            speed: this.clampNumber(getControlValue('speed', this.controls.speed), 1, 100, this.controls.speed),
            cycleSpeed: this.clampNumber(
                getControlValue('cycleSpeed', this.controls.cycleSpeed),
                1,
                100,
                this.controls.cycleSpeed,
            ),
            bCount: Math.round(
                this.clampNumber(getControlValue('bCount', this.controls.bCount), 1, 18, this.controls.bCount),
            ),
        }
    }

    protected applyControls(controls: LavaLampControls): void {
        this.controls = controls
        this.backgroundColor = controls.bgColor

        if (controls.bCount !== this.blobCount) {
            this.resetBlobs(controls.bCount)
        }
    }

    protected async loadResources(): Promise<void> {
        if (this.ctx) this.ctx.imageSmoothingEnabled = true

        this.ensureFieldGrid(this.canvas?.width ?? 320, this.canvas?.height ?? 200)
        this.resetBlobs(this.controls.bCount)
    }

    protected draw(time: number, deltaTime: number): void {
        if (!this.ctx || !this.canvas) return

        const ctx = this.ctx
        const w = this.canvas.width
        const h = this.canvas.height
        const dt = deltaTime > 0 ? Math.min(deltaTime, 0.05) : 1 / 60

        this.ensureFieldGrid(w, h)

        this.bgHue = (this.bgHue + this.controls.cycleSpeed * 1.2 * dt) % 360
        this.lavaHue = (this.lavaHue + this.controls.cycleSpeed * 2.2 * dt) % 360

        this.updateBlobs(time, w, h)

        const palette = this.resolvePalette()

        const backgroundColor = this.controls.bgCycle
            ? this.shiftHexHue(this.controls.bgColor, this.bgHue)
            : this.controls.bgColor
        ctx.fillStyle = backgroundColor
        ctx.fillRect(0, 0, w, h)

        this.drawBackdrop(ctx, w, h, palette)

        const vignette = ctx.createLinearGradient(0, 0, 0, h)
        vignette.addColorStop(0, this.hexToRgba(this.controls.bgColor, 0.17))
        vignette.addColorStop(0.5, this.hexToRgba('#000000', 0.0))
        vignette.addColorStop(1, this.hexToRgba('#000000', 0.2))
        ctx.fillStyle = vignette
        ctx.fillRect(0, 0, w, h)

        const lavaColorA = this.controls.rainbow
            ? this.shiftHexHue(palette.color1, this.lavaHue)
            : palette.color1
        const lavaColorB = this.controls.rainbow
            ? this.shiftHexHue(palette.color2, this.lavaHue + 140)
            : palette.color2
        const lavaColorC = this.controls.rainbow
            ? this.shiftHexHue(palette.color3, this.lavaHue + 280)
            : palette.color3

        const colorA = this.hexToRgb(lavaColorA)
        const colorB = this.hexToRgb(lavaColorB)
        const colorC = this.hexToRgb(lavaColorC)

        this.computeField(w, h)
        this.drawLavaCells(ctx, time, colorA, colorB, colorC)
        this.drawContours(ctx, colorA, colorB, colorC)
    }

    private ensureFieldGrid(width: number, height: number): void {
        const nextCols = Math.floor(width / this.step) + 3
        const nextRows = Math.floor(height / this.step) + 3
        const needsResize = nextCols !== this.cols || nextRows !== this.rows || this.field.length === 0

        if (!needsResize) return

        this.cols = nextCols
        this.rows = nextRows
        this.field = new Float32Array(this.cols * this.rows)
    }

    private resetBlobs(count: number): void {
        const w = this.canvas?.width ?? 320
        const h = this.canvas?.height ?? 200
        const minDim = Math.min(w, h)

        this.blobs = []
        for (let i = 0; i < count; i++) {
            const s1 = this.hash(i * 13.37 + 1.17)
            const s2 = this.hash(i * 19.11 + 4.28)
            const s3 = this.hash(i * 29.87 + 8.72)
            const radius = minDim * (0.072 + s1 * 0.088)

            this.blobs.push({
                x: radius + s2 * Math.max(8, w - radius * 2),
                y: radius + s3 * Math.max(8, h - radius * 2),
                vx: (this.hash(i * 5.91 + 0.43) * 2 - 1) * (0.42 + s1 * 0.62),
                vy: (this.hash(i * 8.27 + 2.83) * 2 - 1) * (0.36 + s2 * 0.78),
                radius,
                phase: this.hash(i * 31.7 + 6.14) * Math.PI * 2,
                seed: s1,
            })
        }

        this.blobCount = count
    }

    private updateBlobs(time: number, width: number, height: number): void {
        const speedScale = this.controls.speed / 25
        const centerX = width * 0.5
        const centerY = height * 0.5

        for (let i = 0; i < this.blobs.length; i++) {
            const blob = this.blobs[i]

            const wobbleX = Math.sin(time * (0.75 + blob.seed * 0.7) + blob.phase) * 0.33
            const wobbleY = Math.cos(time * (0.57 + blob.seed * 0.6) + blob.phase * 1.41) * 0.31

            blob.x += (blob.vx + wobbleX) * speedScale
            blob.y += (blob.vy + wobbleY) * speedScale

            blob.x += (centerX - blob.x) * 0.0015 * speedScale
            blob.y += (centerY - blob.y) * 0.0011 * speedScale

            if (blob.x >= width - blob.radius) {
                if (blob.vx > 0) blob.vx = -blob.vx
                blob.x = width - blob.radius
            } else if (blob.x <= blob.radius) {
                if (blob.vx < 0) blob.vx = -blob.vx
                blob.x = blob.radius
            }

            if (blob.y >= height - blob.radius) {
                if (blob.vy > 0) blob.vy = -blob.vy
                blob.y = height - blob.radius
            } else if (blob.y <= blob.radius) {
                if (blob.vy < 0) blob.vy = -blob.vy
                blob.y = blob.radius
            }
        }
    }

    private computeField(width: number, height: number): void {
        const stride = this.cols
        const bubbleFalloff = 28

        for (let gy = 0; gy < this.rows; gy++) {
            const y = Math.min(height, gy * this.step)
            const row = gy * stride

            for (let gx = 0; gx < this.cols; gx++) {
                const x = Math.min(width, gx * this.step)
                let value = 0

                for (let i = 0; i < this.blobs.length; i++) {
                    const blob = this.blobs[i]
                    const dx = x - blob.x
                    const dy = y - blob.y
                    value += (blob.radius * blob.radius) / (dx * dx + dy * dy + bubbleFalloff)
                }

                this.field[row + gx] = value
            }
        }
    }

    private drawLavaCells(ctx: CanvasRenderingContext2D, time: number, colorA: Rgb, colorB: Rgb, colorC: Rgb): void {
        const stride = this.cols

        for (let gy = 0; gy < this.rows - 1; gy++) {
            for (let gx = 0; gx < this.cols - 1; gx++) {
                const idx = gy * stride + gx
                const v0 = this.field[idx]
                const v1 = this.field[idx + 1]
                const v2 = this.field[idx + stride + 1]
                const v3 = this.field[idx + stride]
                const fieldCenter = (v0 + v1 + v2 + v3) * 0.25

                if (fieldCenter < 0.70) continue

                const verticalMix = gy / Math.max(1, this.rows - 1)
                const flowMix = 0.5 + 0.5 * Math.sin(time * 1.45 + gx * 0.24 - gy * 0.18)
                const mixRatio = this.clamp(verticalMix * 0.6 + flowMix * 0.4, 0, 1)
                const hotCore = this.clamp((fieldCenter - 0.96) / 1.32, 0, 1)

                const base = this.mixRgb(colorA, colorB, mixRatio)
                const baseTone = this.mixRgb(base, colorC, hotCore * 0.68)
                const brightnessBoost = this.clamp((fieldCenter - this.threshold) * 32, 0, 42)

                let band = 0
                if (fieldCenter > 2.18) band = 3
                else if (fieldCenter > 1.52) band = 2
                else if (fieldCenter > 1.02) band = 1

                const brightTone = this.boostRgb(baseTone, brightnessBoost + band * 10)
                const tone = this.enrichRgb(brightTone, 0.16 + hotCore * 0.14 + band * 0.04, -0.04 + hotCore * 0.02)
                const alpha = band === 3 ? 0.94 : band === 2 ? 0.82 : band === 1 ? 0.64 : 0.42

                ctx.fillStyle = `rgba(${tone.r},${tone.g},${tone.b},${alpha.toFixed(3)})`
                ctx.fillRect(gx * this.step, gy * this.step, this.step, this.step)
            }
        }
    }

    private drawContours(ctx: CanvasRenderingContext2D, colorA: Rgb, colorB: Rgb, colorC: Rgb): void {
        const stride = this.cols

        for (let li = 0; li < this.contourLevels.length; li++) {
            const level = this.contourLevels[li]
            const mid = this.mixRgb(colorA, colorB, 0.18 + li * 0.20)
            const tone = this.mixRgb(mid, colorC, 0.28 + li * 0.16)
            const edge = this.enrichRgb(this.boostRgb(tone, 60 + li * 14), 0.12 + li * 0.05, -0.02)

            ctx.strokeStyle = `rgba(${edge.r},${edge.g},${edge.b},${(0.12 + li * 0.07).toFixed(3)})`
            ctx.lineWidth = 0.46 + li * 0.14
            ctx.beginPath()

            for (let gy = 0; gy < this.rows - 1; gy++) {
                for (let gx = 0; gx < this.cols - 1; gx++) {
                    const idx = gy * stride + gx
                    const v0 = this.field[idx]
                    const v1 = this.field[idx + 1]
                    const v2 = this.field[idx + stride + 1]
                    const v3 = this.field[idx + stride]

                    let mask = 0
                    if (v0 > level) mask |= 1
                    if (v1 > level) mask |= 2
                    if (v2 > level) mask |= 4
                    if (v3 > level) mask |= 8

                    if (mask === 0 || mask === 15) continue

                    const x = gx * this.step
                    const y = gy * this.step

                    const topX = x + this.interpolate(v0, v1, level) * this.step
                    const topY = y
                    const rightX = x + this.step
                    const rightY = y + this.interpolate(v1, v2, level) * this.step
                    const bottomX = x + this.interpolate(v3, v2, level) * this.step
                    const bottomY = y + this.step
                    const leftX = x
                    const leftY = y + this.interpolate(v0, v3, level) * this.step

                    switch (mask) {
                        case 1:
                        case 14:
                            this.traceSegment(ctx, leftX, leftY, topX, topY)
                            break
                        case 2:
                        case 13:
                            this.traceSegment(ctx, topX, topY, rightX, rightY)
                            break
                        case 3:
                        case 12:
                            this.traceSegment(ctx, leftX, leftY, rightX, rightY)
                            break
                        case 4:
                        case 11:
                            this.traceSegment(ctx, rightX, rightY, bottomX, bottomY)
                            break
                        case 6:
                        case 9:
                            this.traceSegment(ctx, topX, topY, bottomX, bottomY)
                            break
                        case 7:
                        case 8:
                            this.traceSegment(ctx, leftX, leftY, bottomX, bottomY)
                            break
                        case 5:
                            this.traceSegment(ctx, leftX, leftY, bottomX, bottomY)
                            this.traceSegment(ctx, topX, topY, rightX, rightY)
                            break
                        case 10:
                            this.traceSegment(ctx, leftX, leftY, topX, topY)
                            this.traceSegment(ctx, rightX, rightY, bottomX, bottomY)
                            break
                    }
                }
            }

            ctx.stroke()
        }
    }

    private resolvePalette(): ThemePalette {
        if (this.controls.theme !== 'Custom') {
            return THEME_PALETTES[this.controls.theme] ?? THEME_PALETTES.Custom
        }

        return {
            color1: this.controls.color1,
            color2: this.controls.color2,
            color3: this.controls.color3,
        }
    }

    private drawBackdrop(ctx: CanvasRenderingContext2D, width: number, height: number, palette: ThemePalette): void {
        const colorA = this.hexToRgb(palette.color1)
        const colorB = this.hexToRgb(palette.color2)
        const colorC = this.hexToRgb(palette.color3)

        const wash = ctx.createRadialGradient(width * 0.34, height * 0.24, 0, width * 0.34, height * 0.24, width * 0.72)
        wash.addColorStop(0, `rgba(${colorA.r},${colorA.g},${colorA.b},0.16)`)
        wash.addColorStop(0.52, `rgba(${colorB.r},${colorB.g},${colorB.b},0.08)`)
        wash.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = wash
        ctx.fillRect(0, 0, width, height)

        const haze = ctx.createLinearGradient(0, height, width, 0)
        haze.addColorStop(0, `rgba(${colorC.r},${colorC.g},${colorC.b},0.09)`)
        haze.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = haze
        ctx.fillRect(0, 0, width, height)
    }

    private traceSegment(ctx: CanvasRenderingContext2D, x1: number, y1: number, x2: number, y2: number): void {
        ctx.moveTo(x1, y1)
        ctx.lineTo(x2, y2)
    }

    private interpolate(a: number, b: number, threshold: number): number {
        const denom = b - a
        if (Math.abs(denom) < 0.00001) return 0.5
        return this.clamp((threshold - a) / denom, 0, 1)
    }

    private coerceBoolean(value: unknown, fallback: boolean): boolean {
        if (typeof value === 'boolean') return value
        if (typeof value === 'number') return value !== 0
        if (typeof value === 'string') {
            const normalized = value.trim().toLowerCase()
            if (normalized === 'true' || normalized === '1' || normalized === 'yes' || normalized === 'on') {
                return true
            }
            if (normalized === 'false' || normalized === '0' || normalized === 'no' || normalized === 'off') {
                return false
            }
        }
        return fallback
    }

    private clampNumber(value: unknown, min: number, max: number, fallback: number): number {
        const numeric = typeof value === 'number' ? value : Number(value)
        if (!Number.isFinite(numeric)) return fallback
        return Math.max(min, Math.min(max, numeric))
    }

    private normalizeHexColor(value: unknown, fallback: string): string {
        if (typeof value !== 'string') return fallback
        const hex = value.trim()
        if (/^#[0-9a-fA-F]{3}$/.test(hex) || /^#[0-9a-fA-F]{6}$/.test(hex)) {
            return hex.toLowerCase()
        }
        return fallback
    }

    private hexToRgb(hex: string): Rgb {
        const normalized = hex.replace('#', '')
        const full = normalized.length === 3
            ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
            : normalized
        const value = Number.parseInt(full, 16)

        return {
            r: (value >> 16) & 255,
            g: (value >> 8) & 255,
            b: value & 255,
        }
    }

    private hexToRgba(hex: string, alpha: number): string {
        const rgb = this.hexToRgb(hex)
        return `rgba(${rgb.r},${rgb.g},${rgb.b},${this.clamp(alpha, 0, 1).toFixed(3)})`
    }

    private mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
        const ratio = this.clamp(t, 0, 1)
        return {
            r: Math.round(a.r + (b.r - a.r) * ratio),
            g: Math.round(a.g + (b.g - a.g) * ratio),
            b: Math.round(a.b + (b.b - a.b) * ratio),
        }
    }

    private boostRgb(color: Rgb, amount: number): Rgb {
        return {
            r: Math.min(255, Math.round(color.r + amount)),
            g: Math.min(255, Math.round(color.g + amount)),
            b: Math.min(255, Math.round(color.b + amount)),
        }
    }

    private enrichRgb(color: Rgb, saturationBoost: number, lightnessOffset = 0): Rgb {
        const { h, s, l } = this.rgbToHsl(color)
        return this.hslToRgb(
            h,
            this.clamp(s + saturationBoost, 0, 1),
            this.clamp(l + lightnessOffset, 0, 1),
        )
    }

    private shiftHexHue(hex: string, deltaDegrees: number): string {
        const { h, s, l } = this.rgbToHsl(this.hexToRgb(hex))
        return this.hslToHex((h + deltaDegrees + 360) % 360, s, l)
    }

    private rgbToHsl(rgb: Rgb): { h: number; s: number; l: number } {
        const r = rgb.r / 255
        const g = rgb.g / 255
        const b = rgb.b / 255

        const max = Math.max(r, g, b)
        const min = Math.min(r, g, b)
        const delta = max - min
        const l = (max + min) * 0.5

        if (delta === 0) {
            return { h: 0, s: 0, l }
        }

        const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
        let h = 0

        if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
        else if (max === g) h = (b - r) / delta + 2
        else h = (r - g) / delta + 4

        return { h: h * 60, s, l }
    }

    private hslToHex(h: number, s: number, l: number): string {
        const c = (1 - Math.abs(2 * l - 1)) * s
        const hp = h / 60
        const x = c * (1 - Math.abs((hp % 2) - 1))

        let r = 0
        let g = 0
        let b = 0

        if (hp >= 0 && hp < 1) [r, g, b] = [c, x, 0]
        else if (hp < 2) [r, g, b] = [x, c, 0]
        else if (hp < 3) [r, g, b] = [0, c, x]
        else if (hp < 4) [r, g, b] = [0, x, c]
        else if (hp < 5) [r, g, b] = [x, 0, c]
        else [r, g, b] = [c, 0, x]

        const m = l - c / 2
        const toHex = (value: number) => Math.round((value + m) * 255).toString(16).padStart(2, '0')

        return `#${toHex(r)}${toHex(g)}${toHex(b)}`
    }

    private hslToRgb(h: number, s: number, l: number): Rgb {
        const c = (1 - Math.abs(2 * l - 1)) * s
        const hp = h / 60
        const x = c * (1 - Math.abs((hp % 2) - 1))

        let r = 0
        let g = 0
        let b = 0

        if (hp >= 0 && hp < 1) [r, g, b] = [c, x, 0]
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

    private hash(n: number): number {
        const value = Math.sin(n * 127.1) * 43758.5453123
        return value - Math.floor(value)
    }

    private clamp(value: number, min: number, max: number): number {
        return Math.max(min, Math.min(max, value))
    }
}

const effect = new LavaLamp()
initializeEffect(() => effect.initialize(), { instance: effect })
