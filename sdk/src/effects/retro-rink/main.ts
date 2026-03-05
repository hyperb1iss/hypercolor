import 'reflect-metadata'
import {
    CanvasEffect,
    ColorControl,
    ComboboxControl,
    Effect,
    NumberControl,
    getControlValue,
    initializeEffect,
    normalizeSpeed,
} from '@hypercolor/sdk'

interface RetroRinkControls {
    speed: number
    density: number
    lineWidth: number
    scene: string
    colorMode: string
    cycleSpeed: number
    frontColor: string
    accentColor: string
    background: string
}

interface Motif {
    x: number
    y: number
    size: number
    phase: number
    seed: number
}

const SCENES = ['Loop Carpet', 'Confetti Drift', 'Neon Maze']
const COLOR_MODES = ['Static', 'Color Cycle']

@Effect({
    name: 'Retro Roller Rink',
    description: 'Roller-rink carpet vibes with looping squiggles, confetti motifs, and neon geometry',
    author: 'Hypercolor',
    audioReactive: false,
})
class RetroRollerRink extends CanvasEffect<RetroRinkControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Pattern movement speed' })
    speed!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 62, tooltip: 'Pattern motif amount' })
    density!: number

    @NumberControl({ label: 'Line Width', min: 1, max: 8, default: 3, tooltip: 'Stroke thickness' })
    lineWidth!: number

    @ComboboxControl({ label: 'Scene', values: SCENES, default: 'Loop Carpet', tooltip: 'Pattern family' })
    scene!: string

    @ComboboxControl({ label: 'Color Mode', values: COLOR_MODES, default: 'Color Cycle', tooltip: 'Static or cycling colors' })
    colorMode!: string

    @NumberControl({ label: 'Cycle Speed', min: 0, max: 100, default: 44, tooltip: 'Color cycling speed' })
    cycleSpeed!: number

    @ColorControl({ label: 'Front Color', default: '#00d4a0', tooltip: 'Primary motif color' })
    frontColor!: string

    @ColorControl({ label: 'Accent Color', default: '#f542ff', tooltip: 'Secondary motif color' })
    accentColor!: string

    @ColorControl({ label: 'Background', default: '#180022', tooltip: 'Background color' })
    background!: string

    private controlState: RetroRinkControls = {
        speed: normalizeSpeed(5),
        density: 62,
        lineWidth: 3,
        scene: 'Loop Carpet',
        colorMode: 'Color Cycle',
        cycleSpeed: 44,
        frontColor: '#00d4a0',
        accentColor: '#f542ff',
        background: '#180022',
    }

    private motifs: Motif[] = []
    private motifCount = 0

    constructor() {
        super({ id: 'retro-roller-rink', name: 'Retro Roller Rink', backgroundColor: '#180022' })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.density = getControlValue('density', 62)
        this.lineWidth = getControlValue('lineWidth', 3)
        this.scene = getControlValue('scene', 'Loop Carpet')
        this.colorMode = getControlValue('colorMode', 'Color Cycle')
        this.cycleSpeed = getControlValue('cycleSpeed', 44)
        this.frontColor = getControlValue('frontColor', '#00d4a0')
        this.accentColor = getControlValue('accentColor', '#f542ff')
        this.background = getControlValue('background', '#180022')
    }

    protected getControlValues(): RetroRinkControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            density: getControlValue('density', 62),
            lineWidth: getControlValue('lineWidth', 3),
            scene: getControlValue('scene', 'Loop Carpet'),
            colorMode: getControlValue('colorMode', 'Color Cycle'),
            cycleSpeed: getControlValue('cycleSpeed', 44),
            frontColor: getControlValue('frontColor', '#00d4a0'),
            accentColor: getControlValue('accentColor', '#f542ff'),
            background: getControlValue('background', '#180022'),
        }
    }

    protected applyControls(controls: RetroRinkControls): void {
        this.controlState = controls
        this.backgroundColor = controls.background
        const nextCount = this.computeMotifCount(controls.density)
        if (nextCount !== this.motifCount) {
            this.generateMotifs(nextCount)
        }
    }

    protected async loadResources(): Promise<void> {
        this.generateMotifs(this.computeMotifCount(this.controlState.density))
    }

    protected draw(time: number): void {
        if (!this.ctx || !this.canvas) return

        const ctx = this.ctx
        const w = this.canvas.width
        const h = this.canvas.height

        const t = time * this.controlState.speed
        const colorShift = this.controlState.colorMode === 'Color Cycle' ? t * this.controlState.cycleSpeed * 0.12 : 0

        // Subtle vignette/gradient for depth
        const gradient = ctx.createLinearGradient(0, 0, 0, h)
        gradient.addColorStop(0, this.shiftHexHue(this.controlState.background, -8))
        gradient.addColorStop(1, this.shiftHexHue(this.controlState.background, 10))
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, w, h)

        // Speckle noise layer (classic carpet texture)
        this.drawSpeckleLayer(ctx, w, h, t, colorShift)

        const scene = this.controlState.scene
        if (scene === 'Confetti Drift') {
            this.drawConfettiScene(ctx, w, h, t, colorShift)
        } else if (scene === 'Neon Maze') {
            this.drawMazeScene(ctx, w, h, t, colorShift)
        } else {
            this.drawLoopCarpetScene(ctx, w, h, t, colorShift)
        }
    }

    private computeMotifCount(density: number): number {
        return Math.max(22, Math.floor(24 + density * 0.9))
    }

    private generateMotifs(count: number): void {
        this.motifCount = count
        this.motifs = []
        for (let i = 0; i < count; i++) {
            const s = this.hash(i * 1.73 + 4.1)
            const s2 = this.hash(i * 3.31 + 9.2)
            const s3 = this.hash(i * 7.19 + 2.3)
            this.motifs.push({
                x: s,
                y: s2,
                size: 6 + s3 * 20,
                phase: this.hash(i * 11.7 + 3.9) * Math.PI * 2,
                seed: this.hash(i * 19.4 + 1.2),
            })
        }
    }

    private drawSpeckleLayer(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        t: number,
        hueShift: number,
    ): void {
        const count = Math.floor(80 + this.controlState.density * 2.4)
        for (let i = 0; i < count; i++) {
            const s = this.hash(i * 1.37 + 0.3)
            const s2 = this.hash(i * 2.21 + 2.9)
            const x = s * w
            const y = s2 * h
            const twinkle = 0.2 + 0.8 * (0.5 + 0.5 * Math.sin(t * 0.9 + i * 1.9))
            const radius = 0.5 + this.hash(i * 5.31 + 7.8) * 1.4
            const useFront = i % 2 === 0
            const base = useFront ? this.controlState.frontColor : this.controlState.accentColor
            ctx.fillStyle = this.hexToRgba(this.shiftHexHue(base, hueShift + (useFront ? 8 : -8)), 0.12 * twinkle)
            ctx.beginPath()
            ctx.arc(x, y, radius, 0, Math.PI * 2)
            ctx.fill()
        }
    }

    private drawLoopCarpetScene(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        t: number,
        hueShift: number,
    ): void {
        for (let i = 0; i < this.motifs.length; i++) {
            const m = this.motifs[i]
            const px = (m.x * w + Math.sin(t * 0.65 + m.phase) * 16 + w) % w
            const py = (m.y * h + Math.cos(t * 0.4 + m.phase * 0.8) * 10 + h) % h
            const r = m.size * (0.8 + 0.4 * Math.sin(t * 0.7 + m.phase))

            const colorA = this.shiftHexHue(this.controlState.frontColor, hueShift + m.seed * 40)
            const colorB = this.shiftHexHue(this.controlState.accentColor, hueShift - m.seed * 30)

            ctx.lineCap = 'round'
            ctx.lineJoin = 'round'
            ctx.lineWidth = this.controlState.lineWidth + (m.seed > 0.7 ? 1 : 0)

            // Big looping squiggle
            ctx.strokeStyle = this.hexToRgba(colorA, 0.62)
            ctx.beginPath()
            ctx.moveTo(px - r * 0.9, py - r * 0.35)
            ctx.quadraticCurveTo(px + r * 0.2, py - r * 1.1, px + r * 0.85, py - r * 0.1)
            ctx.quadraticCurveTo(px + r * 0.2, py + r * 0.95, px - r * 0.75, py + r * 0.25)
            ctx.stroke()

            // Accent loop
            ctx.strokeStyle = this.hexToRgba(colorB, 0.5)
            ctx.beginPath()
            ctx.moveTo(px - r * 0.4, py - r * 0.1)
            ctx.quadraticCurveTo(px + r * 0.4, py + r * 0.3, px - r * 0.1, py + r * 0.7)
            ctx.stroke()
        }
    }

    private drawConfettiScene(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        t: number,
        hueShift: number,
    ): void {
        for (let i = 0; i < this.motifs.length; i++) {
            const m = this.motifs[i]
            const px = (m.x * w + Math.sin(t * 0.9 + m.phase) * 20 + w) % w
            const py = (m.y * h + t * (12 + m.seed * 20) + h) % h
            const size = 2 + m.size * 0.35

            const front = this.shiftHexHue(this.controlState.frontColor, hueShift + m.seed * 25)
            const accent = this.shiftHexHue(this.controlState.accentColor, hueShift - m.seed * 25)
            const useAccent = i % 3 === 0
            const col = useAccent ? accent : front

            if (i % 4 === 0) {
                this.drawTriangle(ctx, px, py, size * 1.4, this.hexToRgba(col, 0.65), t + m.phase)
            } else if (i % 4 === 1) {
                ctx.fillStyle = this.hexToRgba(col, 0.55)
                ctx.fillRect(px - size, py - size * 0.35, size * 2, size * 0.7)
            } else {
                ctx.fillStyle = this.hexToRgba(col, 0.45)
                ctx.beginPath()
                ctx.arc(px, py, size * 0.6, 0, Math.PI * 2)
                ctx.fill()
            }
        }
    }

    private drawMazeScene(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        t: number,
        hueShift: number,
    ): void {
        const cell = Math.max(18, 34 - Math.floor(this.controlState.density * 0.14))
        const rows = Math.ceil(h / cell)
        const cols = Math.ceil(w / cell)

        ctx.lineWidth = Math.max(1.5, this.controlState.lineWidth - 0.5)
        ctx.lineCap = 'round'

        for (let gy = 0; gy < rows; gy++) {
            for (let gx = 0; gx < cols; gx++) {
                const seed = this.hash(gx * 12.1 + gy * 7.3 + 3.9)
                const cx = gx * cell + cell * 0.5
                const cy = gy * cell + cell * 0.5
                const phase = t * 0.65 + seed * 6.28
                const rot = ((seed > 0.66 ? 1 : 0) + (Math.sin(phase) > 0.2 ? 1 : 0)) % 2

                const front = this.shiftHexHue(this.controlState.frontColor, hueShift + seed * 30)
                const accent = this.shiftHexHue(this.controlState.accentColor, hueShift - seed * 40)
                const col = seed > 0.5 ? front : accent
                ctx.strokeStyle = this.hexToRgba(col, 0.55)

                ctx.beginPath()
                if (rot === 0) {
                    ctx.moveTo(cx - cell * 0.35, cy - cell * 0.35)
                    ctx.lineTo(cx + cell * 0.35, cy + cell * 0.35)
                } else {
                    ctx.moveTo(cx + cell * 0.35, cy - cell * 0.35)
                    ctx.lineTo(cx - cell * 0.35, cy + cell * 0.35)
                }
                ctx.stroke()
            }
        }
    }

    private drawTriangle(
        ctx: CanvasRenderingContext2D,
        x: number,
        y: number,
        size: number,
        color: string,
        rotation: number,
    ): void {
        ctx.save()
        ctx.translate(x, y)
        ctx.rotate(rotation)
        ctx.fillStyle = color
        ctx.beginPath()
        ctx.moveTo(0, -size)
        ctx.lineTo(size * 0.86, size * 0.5)
        ctx.lineTo(-size * 0.86, size * 0.5)
        ctx.closePath()
        ctx.fill()
        ctx.restore()
    }

    private hash(n: number): number {
        const v = Math.sin(n * 43758.5453123) * 43758.5453123
        return v - Math.floor(v)
    }

    private hexToRgba(hex: string, alpha: number): string {
        const c = this.hexToRgb(hex)
        return `rgba(${c.r}, ${c.g}, ${c.b}, ${Math.max(0, Math.min(1, alpha)).toFixed(3)})`
    }

    private shiftHexHue(hex: string, deltaDegrees: number): string {
        const { h, s, l } = this.rgbToHsl(this.hexToRgb(hex))
        return this.hslToHex((h + deltaDegrees + 360) % 360, s, l)
    }

    private hexToRgb(hex: string): { r: number; g: number; b: number } {
        const norm = hex.replace('#', '')
        const full = norm.length === 3
            ? `${norm[0]}${norm[0]}${norm[1]}${norm[1]}${norm[2]}${norm[2]}`
            : norm
        const int = parseInt(full, 16)
        return {
            r: (int >> 16) & 255,
            g: (int >> 8) & 255,
            b: int & 255,
        }
    }

    private rgbToHsl(rgb: { r: number; g: number; b: number }): { h: number; s: number; l: number } {
        const r = rgb.r / 255
        const g = rgb.g / 255
        const b = rgb.b / 255
        const max = Math.max(r, g, b)
        const min = Math.min(r, g, b)
        const d = max - min
        const l = (max + min) * 0.5

        if (d === 0) return { h: 0, s: 0, l }

        const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
        let h = 0
        if (max === r) h = (g - b) / d + (g < b ? 6 : 0)
        else if (max === g) h = (b - r) / d + 2
        else h = (r - g) / d + 4
        h *= 60

        return { h, s, l }
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
        const toHex = (v: number) => {
            const n = Math.round((v + m) * 255)
            return n.toString(16).padStart(2, '0')
        }

        return `#${toHex(r)}${toHex(g)}${toHex(b)}`
    }
}

const effect = new RetroRollerRink()
initializeEffect(() => effect.initialize(), { instance: effect })
