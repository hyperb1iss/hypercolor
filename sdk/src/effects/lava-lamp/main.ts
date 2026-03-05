import 'reflect-metadata'
import {
    BooleanControl,
    CanvasEffect,
    ColorControl,
    Effect,
    NumberControl,
    getControlValue,
    initializeEffect,
} from '@hypercolor/sdk'

interface LavaLampControls {
    bgColor: string
    bgCycle: boolean
    color1: string
    color2: string
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

    @ColorControl({ label: 'Color 1', default: '#000ded', tooltip: 'Primary lava color' })
    color1!: string

    @ColorControl({ label: 'Color 2', default: '#e40020', tooltip: 'Secondary lava color' })
    color2!: string

    @BooleanControl({ label: 'Lava Color Cycle', default: false, tooltip: 'Cycle lava hues continuously' })
    rainbow!: boolean

    @NumberControl({ label: 'Speed', min: 1, max: 100, default: 22, tooltip: 'Blob movement speed' })
    speed!: number

    @NumberControl({ label: 'Color Speed', min: 1, max: 100, default: 22, tooltip: 'Hue cycling speed' })
    cycleSpeed!: number

    @NumberControl({ label: 'Number of Blobs', min: 1, max: 20, default: 7, tooltip: 'How many metaballs are active' })
    bCount!: number

    private controls: LavaLampControls = {
        bgColor: '#0b0312',
        bgCycle: false,
        color1: '#000ded',
        color2: '#e40020',
        rainbow: false,
        speed: 22,
        cycleSpeed: 22,
        bCount: 7,
    }

    private blobs: Blob[] = []
    private blobCount = 0
    private bgHue = 0
    private lavaHue = 0

    private field = new Float32Array(0)
    private cols = 0
    private rows = 0

    private readonly step = 5
    private readonly threshold = 1.0
    private readonly contourLevels = [1.0, 1.35, 1.75]

    constructor() {
        super({ id: 'lava-lamp', name: 'Lava Lamp', backgroundColor: '#0b0312' })
    }

    protected initializeControls(): void {
        this.bgColor = getControlValue('bgColor', '#0b0312')
        this.bgCycle = getControlValue('bgCycle', false)
        this.color1 = getControlValue('color1', '#000ded')
        this.color2 = getControlValue('color2', '#e40020')
        this.rainbow = getControlValue('rainbow', false)
        this.speed = getControlValue('speed', 22)
        this.cycleSpeed = getControlValue('cycleSpeed', 22)
        this.bCount = getControlValue('bCount', 7)
    }

    protected getControlValues(): LavaLampControls {
        return {
            bgColor: this.normalizeHexColor(getControlValue('bgColor', this.controls.bgColor), this.controls.bgColor),
            bgCycle: this.coerceBoolean(getControlValue('bgCycle', this.controls.bgCycle), this.controls.bgCycle),
            color1: this.normalizeHexColor(getControlValue('color1', this.controls.color1), this.controls.color1),
            color2: this.normalizeHexColor(getControlValue('color2', this.controls.color2), this.controls.color2),
            rainbow: this.coerceBoolean(getControlValue('rainbow', this.controls.rainbow), this.controls.rainbow),
            speed: this.clampNumber(getControlValue('speed', this.controls.speed), 1, 100, this.controls.speed),
            cycleSpeed: this.clampNumber(
                getControlValue('cycleSpeed', this.controls.cycleSpeed),
                1,
                100,
                this.controls.cycleSpeed,
            ),
            bCount: Math.round(
                this.clampNumber(getControlValue('bCount', this.controls.bCount), 1, 20, this.controls.bCount),
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
        if (this.ctx) this.ctx.imageSmoothingEnabled = false

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

        const backgroundColor = this.controls.bgCycle
            ? this.shiftHexHue(this.controls.bgColor, this.bgHue)
            : this.controls.bgColor
        ctx.fillStyle = backgroundColor
        ctx.fillRect(0, 0, w, h)

        const vignette = ctx.createLinearGradient(0, 0, 0, h)
        vignette.addColorStop(0, this.hexToRgba(this.controls.bgColor, 0.17))
        vignette.addColorStop(0.5, this.hexToRgba('#000000', 0.0))
        vignette.addColorStop(1, this.hexToRgba('#000000', 0.2))
        ctx.fillStyle = vignette
        ctx.fillRect(0, 0, w, h)

        const lavaColorA = this.controls.rainbow
            ? this.shiftHexHue(this.controls.color1, this.lavaHue)
            : this.controls.color1
        const lavaColorB = this.controls.rainbow
            ? this.shiftHexHue(this.controls.color2, this.lavaHue + 180)
            : this.controls.color2

        const colorA = this.hexToRgb(lavaColorA)
        const colorB = this.hexToRgb(lavaColorB)

        this.computeField(w, h)
        this.drawLavaCells(ctx, time, colorA, colorB)
        this.drawContours(ctx, colorA, colorB)
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
            const radius = minDim * (0.058 + s1 * 0.072)

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
        const bubbleFalloff = 36

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

    private drawLavaCells(ctx: CanvasRenderingContext2D, time: number, colorA: Rgb, colorB: Rgb): void {
        const stride = this.cols

        for (let gy = 0; gy < this.rows - 1; gy++) {
            for (let gx = 0; gx < this.cols - 1; gx++) {
                const idx = gy * stride + gx
                const v0 = this.field[idx]
                const v1 = this.field[idx + 1]
                const v2 = this.field[idx + stride + 1]
                const v3 = this.field[idx + stride]
                const fieldCenter = (v0 + v1 + v2 + v3) * 0.25

                if (fieldCenter < 0.78) continue

                const verticalMix = gy / Math.max(1, this.rows - 1)
                const flowMix = 0.5 + 0.5 * Math.sin(time * 1.45 + gx * 0.24 - gy * 0.18)
                const mixRatio = this.clamp(verticalMix * 0.6 + flowMix * 0.4, 0, 1)

                const base = this.mixRgb(colorA, colorB, mixRatio)
                const brightnessBoost = this.clamp((fieldCenter - this.threshold) * 34, 0, 46)

                let band = 0
                if (fieldCenter > 2.45) band = 3
                else if (fieldCenter > 1.72) band = 2
                else if (fieldCenter > 1.18) band = 1

                const tone = this.boostRgb(base, brightnessBoost + band * 18)
                const alpha = band === 3 ? 0.97 : band === 2 ? 0.84 : band === 1 ? 0.66 : 0.44

                ctx.fillStyle = `rgba(${tone.r},${tone.g},${tone.b},${alpha.toFixed(3)})`
                ctx.fillRect(gx * this.step, gy * this.step, this.step + 1, this.step + 1)
            }
        }
    }

    private drawContours(ctx: CanvasRenderingContext2D, colorA: Rgb, colorB: Rgb): void {
        const stride = this.cols

        for (let li = 0; li < this.contourLevels.length; li++) {
            const level = this.contourLevels[li]
            const tone = this.mixRgb(colorA, colorB, 0.18 + li * 0.32)
            const edge = this.boostRgb(tone, 72 + li * 16)

            ctx.strokeStyle = `rgba(${edge.r},${edge.g},${edge.b},${(0.36 + li * 0.12).toFixed(3)})`
            ctx.lineWidth = 1.1 + li * 0.35
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
