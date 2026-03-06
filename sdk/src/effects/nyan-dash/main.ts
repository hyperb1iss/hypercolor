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

interface NyanDashControls {
    animationSpeed: number
    scale: number
    positionX: number
    positionY: number
    trailMode: string
    colorCycle: boolean
    cycleSpeed: number
    starDensity: number
}

interface DashStar {
    x: number
    y: number
    size: number
    twinkle: number
    drift: number
    seed: number
    hueOffset: number
}

const TRAIL_MODES = ['Classic', 'Pulse', 'Comet']

@Effect({
    name: 'Nyan Dash',
    description: 'Playful stylized cat dash with rainbow trail variants, star pops, and smooth looping motion',
    author: 'Hypercolor',
    audioReactive: false,
})
class NyanDash extends CanvasEffect<NyanDashControls> {
    @NumberControl({ label: 'Animation Speed', min: 1, max: 10, default: 6, tooltip: 'Overall motion speed' })
    animationSpeed!: number

    @NumberControl({ label: 'Scale', min: 40, max: 180, default: 100, tooltip: 'Cat and trail size' })
    scale!: number

    @NumberControl({ label: 'Position X', min: -100, max: 100, default: 0, tooltip: 'Horizontal offset' })
    positionX!: number

    @NumberControl({ label: 'Position Y', min: -100, max: 100, default: 0, tooltip: 'Vertical offset' })
    positionY!: number

    @ComboboxControl({ label: 'Trail Mode', values: TRAIL_MODES, default: 'Classic', tooltip: 'Trail behavior style' })
    trailMode!: string

    @BooleanControl({ label: 'Color Cycle', default: true, tooltip: 'Animate hues through the palette' })
    colorCycle!: boolean

    @NumberControl({ label: 'Cycle Speed', min: 0, max: 100, default: 34, tooltip: 'Hue cycle speed' })
    cycleSpeed!: number

    @NumberControl({ label: 'Star Density', min: 0, max: 100, default: 44, tooltip: 'Twinkling star amount' })
    starDensity!: number

    private controls: NyanDashControls = {
        animationSpeed: normalizeSpeed(6),
        scale: 100,
        positionX: 0,
        positionY: 0,
        trailMode: 'Classic',
        colorCycle: true,
        cycleSpeed: 34,
        starDensity: 44,
    }

    private stars: DashStar[] = []
    private starCount = 0
    private canvasWidth = 0
    private canvasHeight = 0

    constructor() {
        super({ id: 'nyan-dash', name: 'Nyan Dash', backgroundColor: '#040513' })
    }

    protected initializeControls(): void {
        this.animationSpeed = getControlValue('animationSpeed', 6)
        this.scale = getControlValue('scale', 100)
        this.positionX = getControlValue('positionX', 0)
        this.positionY = getControlValue('positionY', 0)
        this.trailMode = getControlValue('trailMode', 'Classic')
        this.colorCycle = getControlValue('colorCycle', true)
        this.cycleSpeed = getControlValue('cycleSpeed', 34)
        this.starDensity = getControlValue('starDensity', 44)
    }

    protected getControlValues(): NyanDashControls {
        return {
            animationSpeed: normalizeSpeed(getControlValue('animationSpeed', 6)),
            scale: getControlValue('scale', 100),
            positionX: getControlValue('positionX', 0),
            positionY: getControlValue('positionY', 0),
            trailMode: getControlValue('trailMode', 'Classic'),
            colorCycle: this.coerceBoolean(getControlValue('colorCycle', true), true),
            cycleSpeed: getControlValue('cycleSpeed', 34),
            starDensity: getControlValue('starDensity', 58),
        }
    }

    protected applyControls(controls: NyanDashControls): void {
        this.controls = {
            ...controls,
            trailMode: TRAIL_MODES.includes(controls.trailMode) ? controls.trailMode : 'Classic',
            scale: this.clamp(controls.scale, 40, 180),
            positionX: this.clamp(controls.positionX, -100, 100),
            positionY: this.clamp(controls.positionY, -100, 100),
            cycleSpeed: this.clamp(controls.cycleSpeed, 0, 100),
            starDensity: this.clamp(controls.starDensity, 0, 100),
        }
        this.syncStars(this.canvas?.width ?? 320, this.canvas?.height ?? 200)
    }

    protected async loadResources(): Promise<void> {
        this.syncStars(this.canvas?.width ?? 320, this.canvas?.height ?? 200, true)
    }

    protected draw(time: number, deltaTime: number): void {
        if (!this.ctx || !this.canvas) return

        const ctx = this.ctx
        const width = this.canvas.width
        const height = this.canvas.height
        const dt = deltaTime > 0 ? Math.min(deltaTime, 0.05) : 1 / 60

        this.syncStars(width, height)

        const speed = this.controls.animationSpeed
        const scale = this.clamp(this.controls.scale / 100, 0.4, 1.8)
        const unit = Math.max(1, Math.round(2 * scale))
        const cycleHue = this.controls.colorCycle ? time * this.controls.cycleSpeed * 0.9 : 0

        this.drawBackdrop(ctx, width, height, cycleHue)
        this.drawStars(ctx, width, height, time, speed, cycleHue)

        const travelPadding = 70 * scale
        const travel = (time * speed * 0.14) % 1
        const loopX = travel * (width + travelPadding * 2) - travelPadding

        const offsetX = (this.controls.positionX / 100) * width * 0.36
        const offsetY = (this.controls.positionY / 100) * height * 0.36
        const bob = Math.sin(time * (2.6 + speed * 0.5)) * 4 * scale

        const catX = loopX + offsetX
        const catY = this.clamp(height * 0.55 + offsetY + bob, 18 * scale, height - 18 * scale)

        const bodyWidth = 20 * unit
        const catLeft = catX - bodyWidth * 0.5 - 2 * unit

        this.drawTrail(ctx, width, catLeft, catY, time, dt, unit, cycleHue)
        this.drawCat(ctx, catX, catY, unit, time, cycleHue)
    }

    private syncStars(width: number, height: number, force = false): void {
        const targetCount = Math.max(0, Math.floor(this.controls.starDensity * 1.25))
        const sizeChanged = this.canvasWidth !== width || this.canvasHeight !== height

        if (!force && !sizeChanged && targetCount === this.starCount) return

        this.canvasWidth = width
        this.canvasHeight = height
        this.starCount = targetCount
        this.stars = []

        for (let i = 0; i < targetCount; i++) {
            const s1 = this.hash(i * 1.87 + 2.17)
            const s2 = this.hash(i * 2.93 + 6.11)
            const s3 = this.hash(i * 4.77 + 9.41)
            const s4 = this.hash(i * 8.13 + 1.29)
            const s5 = this.hash(i * 12.41 + 4.83)
            const s6 = this.hash(i * 16.53 + 5.09)

            this.stars.push({
                x: s1,
                y: s2,
                size: 1 + s3 * 2.3,
                twinkle: 1.1 + s4 * 2.6,
                drift: 4 + s5 * 28,
                seed: s6,
                hueOffset: s4 * 360,
            })
        }
    }

    private drawBackdrop(ctx: CanvasRenderingContext2D, width: number, height: number, cycleHue: number): void {
        const top = this.controls.colorCycle ? this.hslToHex(cycleHue + 234, 72, 10) : '#080b24'
        const bottom = this.controls.colorCycle ? this.hslToHex(cycleHue + 272, 68, 6) : '#03040e'

        const bg = ctx.createLinearGradient(0, 0, 0, height)
        bg.addColorStop(0, top)
        bg.addColorStop(1, bottom)
        ctx.fillStyle = bg
        ctx.fillRect(0, 0, width, height)

        const haze = ctx.createLinearGradient(0, height * 0.4, 0, height)
        haze.addColorStop(0, 'rgba(128, 255, 234, 0.01)')
        haze.addColorStop(1, 'rgba(225, 53, 255, 0.06)')
        ctx.fillStyle = haze
        ctx.fillRect(0, 0, width, height)
    }

    private drawStars(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        speed: number,
        cycleHue: number,
    ): void {
        const driftScale = 0.35 + speed * 0.16

        for (let i = 0; i < this.stars.length; i++) {
            const star = this.stars[i]
            const x = (star.x * width + time * star.drift * driftScale) % width
            const y = star.y * height + Math.sin(time * (0.7 + star.seed) + star.seed * 11.3) * (2 + star.size)
            const twinkle = 0.5 + 0.5 * Math.sin(time * star.twinkle + star.seed * 23.4)
            const alpha = 0.16 + twinkle * 0.82
            const size = Math.max(1, Math.round(star.size + twinkle * 0.8))

            const color = this.controls.colorCycle
                ? this.hslToHex(cycleHue + star.hueOffset, 96, 66)
                : '#ecf2ff'

            ctx.globalAlpha = alpha
            ctx.fillStyle = color
            ctx.fillRect(this.snap(x), this.snap(y), size, size)

            if (twinkle > 0.72) {
                const arm = Math.max(2, Math.round(size * 1.6))
                ctx.fillRect(this.snap(x - arm), this.snap(y), arm * 2 + 1, 1)
                ctx.fillRect(this.snap(x), this.snap(y - arm), 1, arm * 2 + 1)
            }

            if (twinkle > 0.93) {
                const pop = Math.max(2, Math.round(size * 2.2))
                ctx.globalAlpha = 0.25 + alpha * 0.42
                ctx.fillRect(this.snap(x - pop), this.snap(y - pop), pop * 2 + 1, 1)
                ctx.fillRect(this.snap(x - pop), this.snap(y + pop), pop * 2 + 1, 1)
                ctx.fillRect(this.snap(x - pop), this.snap(y - pop), 1, pop * 2 + 1)
                ctx.fillRect(this.snap(x + pop), this.snap(y - pop), 1, pop * 2 + 1)
            }
        }

        ctx.globalAlpha = 1
    }

    private drawTrail(
        ctx: CanvasRenderingContext2D,
        width: number,
        catLeft: number,
        catCenterY: number,
        time: number,
        deltaTime: number,
        unit: number,
        cycleHue: number,
    ): void {
        const trailLength = Math.max(0, catLeft + 4 * unit)
        if (trailLength <= 0) return

        const baseBands = ['#ff3f8e', '#ff8656', '#ffb347', '#74f2a8', '#5dc9ff', '#9380ff']
        const bandHeight = Math.max(1, Math.round(unit * 2.35))
        const top = catCenterY - bandHeight * 3
        const segment = Math.max(2, Math.round(unit * 3.4))
        const mode = this.controls.trailMode
        const pulseClock = time * (6 + this.controls.animationSpeed * 0.7)

        for (let bandIndex = 0; bandIndex < baseBands.length; bandIndex++) {
            const color = this.controls.colorCycle
                ? this.hslToHex(cycleHue + bandIndex * 42, 96, 58)
                : baseBands[bandIndex]
            const yBase = top + bandIndex * bandHeight

            for (let x = -segment * 2; x < trailLength + segment; x += segment) {
                const wave = Math.sin(time * 4.2 + x * 0.048 + bandIndex * 0.72) * bandHeight * 0.2
                let modeWave = wave
                let stretch = 1

                if (mode === 'Pulse') {
                    stretch = 0.7 + 0.32 * (0.5 + 0.5 * Math.sin(pulseClock + x * 0.035 + bandIndex))
                } else if (mode === 'Comet') {
                    modeWave += Math.sin(time * 8.2 + x * 0.11 + bandIndex * 1.4) * bandHeight * 0.42
                    stretch = 0.84 + 0.18 * (0.5 + 0.5 * Math.sin(pulseClock + x * 0.04 + bandIndex * 0.6))
                }

                const h = Math.max(1, Math.round(bandHeight * stretch))
                const y = yBase + modeWave + (bandHeight - h) * 0.5
                const alpha = mode === 'Comet' ? 0.78 + 0.2 * (0.5 + 0.5 * Math.sin(pulseClock + x * 0.06)) : 0.92

                ctx.globalAlpha = alpha
                ctx.fillStyle = color
                ctx.fillRect(this.snap(x), this.snap(y), segment + 1, h)

                if (mode === 'Comet' && (bandIndex === 0 || bandIndex === 5)) {
                    const marker = Math.floor(x / segment) + Math.floor((time + deltaTime) * 15)
                    if (marker % 8 === 0) {
                        ctx.globalAlpha = 0.6
                        const sparkleX = this.snap(x + segment * 0.5)
                        const sparkleY = this.snap(y + h * 0.5)
                        ctx.fillStyle = this.hslToHex(cycleHue + bandIndex * 42, 96, 64)
                        ctx.fillRect(sparkleX - 1, sparkleY, 3, 1)
                        ctx.fillRect(sparkleX, sparkleY - 1, 1, 3)
                    }
                }
            }
        }

        ctx.globalAlpha = 1

        // Fade trail edge into the horizon for cleaner loops.
        const fade = ctx.createLinearGradient(Math.min(width, trailLength), 0, 0, 0)
        fade.addColorStop(0, 'rgba(0, 0, 0, 0)')
        fade.addColorStop(1, 'rgba(0, 0, 0, 0.22)')
        ctx.fillStyle = fade
        ctx.fillRect(0, top - bandHeight, trailLength, bandHeight * 8)
    }

    private drawCat(
        ctx: CanvasRenderingContext2D,
        centerX: number,
        centerY: number,
        unit: number,
        time: number,
        cycleHue: number,
    ): void {
        const outline = '#1e1636'
        const headColor = '#d6dcf8'
        const bodyColor = '#f4d0a3'
        const frosting = this.controls.colorCycle ? this.hslToHex(cycleHue + 332, 88, 70) : '#ff8ed4'
        const frostingShade = this.controls.colorCycle ? this.hslToHex(cycleHue + 320, 84, 62) : '#ff6ec4'

        const bodyW = 20 * unit
        const bodyH = 13 * unit
        const headSize = 10 * unit

        const bodyX = this.snap(centerX - bodyW * 0.5)
        const bodyY = this.snap(centerY - bodyH * 0.5)

        const tailWag = this.snap(Math.sin(time * (7 + this.controls.animationSpeed * 0.45)) * unit)

        // Tail (stepped silhouette keeps visibility high on LED grids).
        this.fillRect(ctx, bodyX - 7 * unit, bodyY + 4 * unit + tailWag, 6 * unit, 3 * unit, outline)
        this.fillRect(ctx, bodyX - 6 * unit, bodyY + 5 * unit + tailWag, 4 * unit, unit, '#bcc2da')

        // Legs
        const stride = Math.sin(time * (8 + this.controls.animationSpeed)) * unit * 0.8
        const legYs = [
            this.snap(bodyY + bodyH - unit + stride * 0.3),
            this.snap(bodyY + bodyH - unit - stride * 0.2),
            this.snap(bodyY + bodyH - unit + stride * 0.15),
            this.snap(bodyY + bodyH - unit - stride * 0.25),
        ]
        const legXs = [bodyX + 2 * unit, bodyX + 7 * unit, bodyX + 12 * unit, bodyX + 17 * unit]

        for (let i = 0; i < legXs.length; i++) {
            this.fillRect(ctx, legXs[i] - unit, legYs[i] - unit, 2 * unit + 1, 3 * unit, outline)
            this.fillRect(ctx, legXs[i], legYs[i], unit, 2 * unit, '#c8cde0')
        }

        // Body pastry
        this.fillRect(ctx, bodyX - unit, bodyY - unit, bodyW + 2 * unit, bodyH + 2 * unit, outline)
        this.fillRect(ctx, bodyX, bodyY, bodyW, bodyH, bodyColor)

        // Frosting slab + inner fill for depth.
        this.fillRect(ctx, bodyX + 2 * unit, bodyY + unit, bodyW - 4 * unit, bodyH - 4 * unit, frosting)
        this.fillRect(ctx, bodyX + 3 * unit, bodyY + 2 * unit, bodyW - 7 * unit, bodyH - 6 * unit, frostingShade)

        const sprinklePalette = this.controls.colorCycle
            ? [
                this.hslToHex(cycleHue + 22, 96, 64),
                this.hslToHex(cycleHue + 120, 94, 70),
                this.hslToHex(cycleHue + 190, 96, 72),
                this.hslToHex(cycleHue + 276, 92, 74),
                this.hslToHex(cycleHue + 340, 94, 72),
            ]
            : ['#ffb347', '#74f3ff', '#7eff9a', '#b7a8ff', '#ffb4d9']

        const sprinkles: Array<[number, number, number]> = [
            [4, 3, 0],
            [8, 2, 1],
            [12, 4, 2],
            [6, 6, 3],
            [10, 7, 4],
            [14, 6, 0],
            [16, 3, 2],
            [5, 8, 1],
        ]

        for (let i = 0; i < sprinkles.length; i++) {
            const [sx, sy, colorIndex] = sprinkles[i]
            this.fillRect(
                ctx,
                bodyX + sx * unit,
                bodyY + sy * unit,
                2 * unit,
                Math.max(1, unit),
                sprinklePalette[colorIndex],
            )
        }

        // Head
        const headX = bodyX + bodyW - 2 * unit
        const headY = bodyY - unit
        this.fillRect(ctx, headX - unit, headY - unit, headSize + 2 * unit, headSize + 2 * unit, outline)
        this.fillRect(ctx, headX, headY, headSize, headSize, headColor)

        // Ears (blocky stepped ears for a custom silhouette).
        this.fillRect(ctx, headX + unit, headY - 4 * unit, 3 * unit, 3 * unit, outline)
        this.fillRect(ctx, headX + 2 * unit, headY - 3 * unit, unit, unit, '#ffc0da')

        this.fillRect(ctx, headX + 6 * unit, headY - 4 * unit, 3 * unit, 3 * unit, outline)
        this.fillRect(ctx, headX + 7 * unit, headY - 3 * unit, unit, unit, '#ffc0da')

        // Face details
        const blink = Math.sin(time * 2.8 + centerX * 0.01) > 0.94
        const eyeColor = '#1a1830'

        if (blink) {
            this.fillRect(ctx, headX + 2 * unit, headY + 4 * unit, 2 * unit, 1, eyeColor)
            this.fillRect(ctx, headX + 6 * unit, headY + 4 * unit, 2 * unit, 1, eyeColor)
        } else {
            this.fillRect(ctx, headX + 2 * unit, headY + 3 * unit, unit, 2 * unit, eyeColor)
            this.fillRect(ctx, headX + 7 * unit, headY + 3 * unit, unit, 2 * unit, eyeColor)
        }

        this.fillRect(ctx, headX + 4 * unit, headY + 5 * unit, 2 * unit, unit, '#ff7bb4')
        this.fillRect(ctx, headX + 3 * unit, headY + 6 * unit, unit, 1, eyeColor)
        this.fillRect(ctx, headX + 6 * unit, headY + 6 * unit, unit, 1, eyeColor)

        // Whiskers
        this.fillRect(ctx, headX - unit, headY + 4 * unit, 2 * unit, 1, outline)
        this.fillRect(ctx, headX - unit, headY + 6 * unit, 2 * unit, 1, outline)
        this.fillRect(ctx, headX + headSize - 1, headY + 4 * unit, 2 * unit, 1, outline)
        this.fillRect(ctx, headX + headSize - 1, headY + 6 * unit, 2 * unit, 1, outline)
    }

    private fillRect(
        ctx: CanvasRenderingContext2D,
        x: number,
        y: number,
        width: number,
        height: number,
        color: string,
    ): void {
        if (width <= 0 || height <= 0) return
        ctx.fillStyle = color
        ctx.fillRect(this.snap(x), this.snap(y), Math.max(1, Math.round(width)), Math.max(1, Math.round(height)))
    }

    private coerceBoolean(value: unknown, fallback: boolean): boolean {
        if (typeof value === 'boolean') return value
        if (typeof value === 'number') return value !== 0
        if (typeof value === 'string') {
            const normalized = value.trim().toLowerCase()
            if (['true', '1', 'yes', 'on'].includes(normalized)) return true
            if (['false', '0', 'no', 'off'].includes(normalized)) return false
        }
        return fallback
    }

    private hslToHex(h: number, s: number, l: number): string {
        h = ((h % 360) + 360) % 360
        s = this.clamp(s, 0, 100) / 100
        l = this.clamp(l, 0, 100) / 100

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

        const toHex = (value: number) => Math.round((value + m) * 255).toString(16).padStart(2, '0')
        return `#${toHex(r)}${toHex(g)}${toHex(b)}`
    }

    private clamp(value: number, min: number, max: number): number {
        return Math.max(min, Math.min(max, value))
    }

    private snap(value: number): number {
        return Math.round(value)
    }

    private hash(value: number): number {
        const seeded = Math.sin(value * 127.1 + 311.7) * 43758.5453123
        return seeded - Math.floor(seeded)
    }
}

const effect = new NyanDash()
initializeEffect(() => effect.initialize(), { instance: effect })
