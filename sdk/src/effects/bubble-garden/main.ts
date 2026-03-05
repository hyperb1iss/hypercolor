import 'reflect-metadata'
import {
    CanvasEffect,
    ColorControl,
    ComboboxControl,
    Effect,
    NumberControl,
    getControlValue,
    initializeEffect,
} from '@hypercolor/sdk'

interface BubbleControls {
    colorMode: string
    bgColor: string
    color: string
    speed: number
    size: number
    count: number
}

interface BubbleParticle {
    x: number
    y: number
    vx: number
    vy: number
    baseSize: number
    alpha: number
}

const COLOR_MODES = ['Single Color', 'Rainbow', 'Color Cycle']

@Effect({
    name: 'Bubble Garden',
    description: 'Community-style bouncing bubbles with crisp rendering and simple controls',
    author: 'Hypercolor',
    audioReactive: false,
})
class BubbleGarden extends CanvasEffect<BubbleControls> {
    @ComboboxControl({
        label: 'Color Mode',
        values: COLOR_MODES,
        default: 'Single Color',
        tooltip: 'Single, rainbow, or cycling bubble color',
    })
    colorMode!: string

    @ColorControl({ label: 'Background Color', default: '#000000', tooltip: 'Canvas background color' })
    bgColor!: string

    @ColorControl({ label: 'Color', default: '#ff0066', tooltip: 'Primary bubble color' })
    color!: string

    @NumberControl({ label: 'Bubble Speed', min: 0, max: 100, default: 10, tooltip: 'Movement speed' })
    speed!: number

    @NumberControl({ label: 'Bubble Size', min: 1, max: 10, default: 5, tooltip: 'Bubble radius scale' })
    size!: number

    @NumberControl({ label: 'Bubble Count', min: 10, max: 120, default: 50, tooltip: 'Number of bubbles' })
    count!: number

    private controls: BubbleControls = {
        colorMode: 'Single Color',
        bgColor: '#000000',
        color: '#ff0066',
        speed: 10,
        size: 5,
        count: 50,
    }

    private bubbles: BubbleParticle[] = []
    private hue = 0
    private bubbleCount = 0

    constructor() {
        super({ id: 'bubble-garden', name: 'Bubble Garden', backgroundColor: '#000000' })
    }

    protected initializeControls(): void {
        this.colorMode = getControlValue('colorMode', 'Single Color')
        this.bgColor = getControlValue('bgColor', '#000000')
        this.color = getControlValue('color', '#ff0066')
        this.speed = getControlValue('speed', 10)
        this.size = getControlValue('size', 5)
        this.count = getControlValue('count', 50)
    }

    protected getControlValues(): BubbleControls {
        return {
            colorMode: getControlValue('colorMode', 'Single Color'),
            bgColor: getControlValue('bgColor', '#000000'),
            color: getControlValue('color', '#ff0066'),
            speed: getControlValue('speed', 10),
            size: getControlValue('size', 5),
            count: getControlValue('count', 50),
        }
    }

    protected applyControls(controls: BubbleControls): void {
        const normalizedCount = Math.max(10, Math.floor(controls.count))
        const speedChanged = controls.speed !== this.controls.speed
        this.controls = { ...controls, count: normalizedCount }
        this.backgroundColor = controls.bgColor

        if (normalizedCount !== this.bubbleCount) {
            this.resetBubbles(normalizedCount)
        } else if (speedChanged) {
            this.randomizeVelocities()
        }
    }

    protected async loadResources(): Promise<void> {
        this.resetBubbles(Math.max(10, Math.floor(this.controls.count)))
    }

    protected draw(_time: number): void {
        if (!this.ctx || !this.canvas) return

        const ctx = this.ctx
        const width = this.canvas.width
        const height = this.canvas.height

        ctx.fillStyle = this.controls.bgColor
        ctx.fillRect(0, 0, width, height)

        const speedScale = Math.max(0, this.controls.speed) / 10
        const sizeScale = Math.max(0.2, this.controls.size / 5)
        this.hue = (this.hue + 1) % 360

        for (let i = 0; i < this.bubbles.length; i++) {
            const b = this.bubbles[i]
            const radius = b.baseSize * sizeScale

            if (speedScale > 0) {
                b.x += b.vx * speedScale
                b.y += b.vy * speedScale

                if (b.x + radius >= width) {
                    b.x = width - radius
                    b.vx = -Math.abs(b.vx)
                } else if (b.x - radius <= 0) {
                    b.x = radius
                    b.vx = Math.abs(b.vx)
                }

                if (b.y + radius >= height) {
                    b.y = height - radius
                    b.vy = -Math.abs(b.vy)
                } else if (b.y - radius <= 0) {
                    b.y = radius
                    b.vy = Math.abs(b.vy)
                }
            }

            const fill = this.resolveColor(this.controls.colorMode, b.x, width)

            ctx.fillStyle = this.hexToRgba(fill, b.alpha)
            ctx.beginPath()
            ctx.arc(b.x, b.y, radius, 0, Math.PI * 2)
            ctx.fill()

            // Crisp rim keeps circles readable on low-resolution RGB layouts.
            ctx.strokeStyle = this.hexToRgba('#ffffff', 0.22)
            ctx.lineWidth = 1
            ctx.beginPath()
            ctx.arc(b.x, b.y, Math.max(1, radius - 0.5), 0, Math.PI * 2)
            ctx.stroke()
        }
    }

    private resolveColor(mode: string, bubbleX: number, width: number): string {
        if (mode === 'Color Cycle') {
            return this.hslToHex(this.hue, 100, 50)
        }

        if (mode === 'Rainbow') {
            const hue = (bubbleX / Math.max(width, 1)) * 360
            return this.hslToHex(hue, 100, 50)
        }

        return this.controls.color
    }

    private resetBubbles(count: number): void {
        const width = this.canvas?.width ?? 320
        const height = this.canvas?.height ?? 200
        this.bubbleCount = count
        this.bubbles = []

        for (let i = 0; i < count; i++) {
            const radius = this.rand(10, 20)
            const alpha = 0.5 + (i / Math.max(1, count - 1)) * 0.4
            this.bubbles.push({
                x: this.rand(radius, Math.max(radius, width - radius)),
                y: this.rand(radius, Math.max(radius, height - radius)),
                vx: this.randomVelocity(),
                vy: this.randomVelocity(),
                baseSize: radius,
                alpha,
            })
        }
    }

    private randomizeVelocities(): void {
        for (let i = 0; i < this.bubbles.length; i++) {
            this.bubbles[i].vx = this.randomVelocity()
            this.bubbles[i].vy = this.randomVelocity()
        }
    }

    private randomVelocity(): number {
        const velocity = this.rand(-10, 10) / 10
        if (Math.abs(velocity) < 0.001) {
            return Math.random() < 0.5 ? -1 : 1
        }
        return velocity
    }

    private rand(min: number, max: number): number {
        return Math.floor(Math.random() * (max - min + 1)) + min
    }

    private hexToRgba(hex: string, alpha: number): string {
        const c = this.hexToRgb(hex)
        return `rgba(${c.r}, ${c.g}, ${c.b}, ${Math.max(0, Math.min(1, alpha)).toFixed(3)})`
    }

    private hexToRgb(hex: string): { r: number; g: number; b: number } {
        const norm = hex.replace('#', '')
        const full = norm.length === 3
            ? `${norm[0]}${norm[0]}${norm[1]}${norm[1]}${norm[2]}${norm[2]}`
            : norm
        const n = parseInt(full, 16)
        return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 }
    }

    private hslToHex(h: number, s: number, l: number): string {
        h = ((h % 360) + 360) % 360
        s /= 100
        l /= 100
        const c = (1 - Math.abs(2 * l - 1)) * s
        const x = c * (1 - Math.abs(((h / 60) % 2) - 1))
        const m = l - c / 2
        let r = 0
        let g = 0
        let b = 0

        if (h < 60) [r, g, b] = [c, x, 0]
        else if (h < 120) [r, g, b] = [x, c, 0]
        else if (h < 180) [r, g, b] = [0, c, x]
        else if (h < 240) [r, g, b] = [0, x, c]
        else if (h < 300) [r, g, b] = [x, 0, c]
        else [r, g, b] = [c, 0, x]

        const toHex = (v: number) => Math.round((v + m) * 255).toString(16).padStart(2, '0')
        return `#${toHex(r)}${toHex(g)}${toHex(b)}`
    }
}

const effect = new BubbleGarden()
initializeEffect(() => effect.initialize(), { instance: effect })
