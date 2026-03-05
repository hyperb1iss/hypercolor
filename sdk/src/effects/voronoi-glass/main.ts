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

interface ReactorControls {
    arms: number
    count: number
    particleSize: number
    growth: number
    rotationMode: string
    colorMode: string
    cycleSpeed: number
    background: string
}

interface ParticleSeed {
    orbitOffset: number
    radialOffset: number
    speedBias: number
    sizeBias: number
    jitter: number
    twinkle: number
}

interface RGB {
    r: number
    g: number
    b: number
}

const ROTATION_MODES = ['Clockwise', 'Counter-Clockwise', 'Alternating', 'Pulse']
const COLOR_MODES = ['RGB Split', 'RGB Cycle', 'Prism', 'Mono']
const DEFAULT_BACKGROUND = '#04060f'

@Effect({
    name: 'Voronoi Glass',
    description: 'Community Swirl Reactor style orbital particles with crisp RGB trails',
    author: 'Hypercolor',
    audioReactive: false,
})
class VoronoiGlass extends CanvasEffect<ReactorControls> {
    @NumberControl({ label: 'Arms', min: 1, max: 10, default: 5, tooltip: 'Number of spiral lanes' })
    arms!: number

    @NumberControl({ label: 'Spawn / Count', min: 16, max: 320, default: 150, tooltip: 'Particle population and spawn flow' })
    count!: number

    @NumberControl({ label: 'Particle Size', min: 1, max: 14, default: 4, tooltip: 'Particle radius' })
    particleSize!: number

    @NumberControl({ label: 'Growth', min: 0, max: 100, default: 62, tooltip: 'Outward expansion amount' })
    growth!: number

    @ComboboxControl({
        label: 'Rotation Mode',
        values: ROTATION_MODES,
        default: 'Alternating',
        tooltip: 'Orbit spin direction behavior',
    })
    rotationMode!: string

    @ComboboxControl({
        label: 'Color Mode',
        values: COLOR_MODES,
        default: 'RGB Split',
        tooltip: 'Particle color pattern',
    })
    colorMode!: string

    @NumberControl({ label: 'Cycle Speed', min: 0, max: 100, default: 58, tooltip: 'Rotation and hue cycle speed' })
    cycleSpeed!: number

    @ColorControl({ label: 'Background', default: DEFAULT_BACKGROUND, tooltip: 'Background color' })
    background!: string

    private controls: ReactorControls = {
        arms: 5,
        count: 150,
        particleSize: 4,
        growth: 62,
        rotationMode: 'Alternating',
        colorMode: 'RGB Split',
        cycleSpeed: 58,
        background: DEFAULT_BACKGROUND,
    }

    private particles: ParticleSeed[] = []
    private particleCount = 0

    constructor() {
        super({ id: 'voronoi-glass', name: 'Voronoi Glass', backgroundColor: DEFAULT_BACKGROUND })
    }

    protected initializeControls(): void {
        this.arms = getControlValue('arms', 5)
        this.count = getControlValue('count', 150)
        this.particleSize = getControlValue('particleSize', 4)
        this.growth = getControlValue('growth', 62)
        this.rotationMode = getControlValue('rotationMode', 'Alternating')
        this.colorMode = getControlValue('colorMode', 'RGB Split')
        this.cycleSpeed = getControlValue('cycleSpeed', 58)
        this.background = getControlValue('background', DEFAULT_BACKGROUND)
    }

    protected getControlValues(): ReactorControls {
        return {
            arms: this.clamp(Math.round(getControlValue('arms', this.controls.arms)), 1, 10),
            count: this.clamp(Math.round(getControlValue('count', this.controls.count)), 16, 320),
            particleSize: this.clamp(getControlValue('particleSize', this.controls.particleSize), 1, 14),
            growth: this.clamp(getControlValue('growth', this.controls.growth), 0, 100),
            rotationMode: this.pickValue(getControlValue('rotationMode', this.controls.rotationMode), ROTATION_MODES, 'Alternating'),
            colorMode: this.pickValue(getControlValue('colorMode', this.controls.colorMode), COLOR_MODES, 'RGB Split'),
            cycleSpeed: this.clamp(getControlValue('cycleSpeed', this.controls.cycleSpeed), 0, 100),
            background: this.normalizeHexColor(getControlValue('background', this.controls.background), DEFAULT_BACKGROUND),
        }
    }

    protected async loadResources(): Promise<void> {
        this.ensureParticleCount(this.controls.count)
    }

    protected applyControls(controls: ReactorControls): void {
        this.controls = controls
        this.backgroundColor = controls.background
        this.ensureParticleCount(controls.count)
    }

    protected draw(time: number, _deltaTime: number): void {
        if (!this.ctx || !this.canvas) return

        const ctx = this.ctx
        const w = this.canvas.width
        const h = this.canvas.height
        const cx = w * 0.5
        const cy = h * 0.5
        const minDim = Math.min(w, h)
        const arms = Math.max(1, this.controls.arms)
        const count = this.particles.length
        if (count === 0) return

        const growthMix = this.controls.growth / 100
        const cycleRate = this.controls.cycleSpeed / 100
        const rotationVelocity = 0.4 + cycleRate * 2.2
        const spawnVelocity = 0.65 + cycleRate * 2.1 + count / 220
        const maxRadius = minDim * (0.18 + growthMix * 0.58)
        const coreRadius = minDim * (0.036 + growthMix * 0.02)
        const laneTwist = 2.1 + growthMix * 5.2

        this.drawBacklight(ctx, w, h, time, cycleRate)
        this.drawCore(ctx, cx, cy, coreRadius, time, cycleRate)

        ctx.save()
        ctx.globalCompositeOperation = 'lighter'

        for (let i = 0; i < count; i++) {
            const seed = this.particles[i]
            const arm = i % arms
            const direction = this.resolveDirection(this.controls.rotationMode, arm, time)
            const life = this.fract(time * spawnVelocity * (0.55 + seed.speedBias) + seed.orbitOffset)
            const radialCurve = Math.pow(life, 0.36 + (1 - growthMix) * 0.92)
            const radius = coreRadius + radialCurve * maxRadius + (seed.radialOffset - 0.5) * minDim * 0.045

            const laneBase = (arm / arms) * Math.PI * 2
            const orbital = direction * (time * rotationVelocity + life * Math.PI * 2 * laneTwist)
            const wobble = Math.sin(time * (1.6 + seed.speedBias * 1.9) + seed.twinkle * 9.1) * seed.jitter * 0.42
            const angle = laneBase + orbital + wobble

            const x = cx + Math.cos(angle) * radius
            const y = cy + Math.sin(angle) * radius * (0.9 + seed.jitter * 0.08)
            const color = this.resolveColor(this.controls.colorMode, arm, i, arms, time, life, cycleRate)

            const pulse = 0.62 + 0.38 * Math.sin(time * (4.5 + seed.speedBias * 3.1) + seed.twinkle * 11.0)
            const size = Math.max(0.7, this.controls.particleSize * (0.5 + seed.sizeBias * 1.05) * (0.65 + radialCurve * 0.8))
            const alpha = this.clamp((0.24 + 0.9 * Math.sin(Math.PI * life)) * pulse, 0.08, 1)

            ctx.fillStyle = this.toRgba(color, 0.16 * alpha)
            ctx.beginPath()
            ctx.arc(x, y, size * 2.2, 0, Math.PI * 2)
            ctx.fill()

            ctx.fillStyle = this.toRgba(color, 0.78 * alpha)
            ctx.beginPath()
            ctx.arc(x, y, size, 0, Math.PI * 2)
            ctx.fill()

            ctx.fillStyle = `rgba(255,255,255,${(0.22 * alpha).toFixed(3)})`
            ctx.beginPath()
            ctx.arc(x, y, Math.max(0.55, size * 0.28), 0, Math.PI * 2)
            ctx.fill()
        }

        ctx.restore()
    }

    private drawBacklight(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        cycleRate: number,
    ): void {
        const centerX = width * (0.5 + Math.sin(time * 0.16) * 0.04)
        const centerY = height * (0.5 + Math.cos(time * 0.18) * 0.04)
        const radius = Math.max(width, height) * 0.84
        const hue = (210 + time * (16 + cycleRate * 70)) % 360
        const base = this.hslToRgb(hue, 82, 53)
        const accent = this.hslToRgb((hue + 130) % 360, 86, 58)

        const glow = ctx.createRadialGradient(centerX, centerY, 0, centerX, centerY, radius)
        glow.addColorStop(0, this.toRgba(base, 0.11))
        glow.addColorStop(0.52, this.toRgba(accent, 0.05))
        glow.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = glow
        ctx.fillRect(0, 0, width, height)
    }

    private drawCore(
        ctx: CanvasRenderingContext2D,
        cx: number,
        cy: number,
        coreRadius: number,
        time: number,
        cycleRate: number,
    ): void {
        const hue = (time * (40 + cycleRate * 90) + 6) % 360
        const coreA = this.hslToRgb(hue, 94, 60)
        const coreB = this.hslToRgb((hue + 150) % 360, 90, 54)
        const pulse = 1 + Math.sin(time * (4.2 + cycleRate * 4.8)) * 0.16
        const radius = coreRadius * pulse

        const coreGradient = ctx.createRadialGradient(cx, cy, 0, cx, cy, radius * 3.1)
        coreGradient.addColorStop(0, this.toRgba(coreA, 0.72))
        coreGradient.addColorStop(0.35, this.toRgba(coreB, 0.38))
        coreGradient.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = coreGradient
        ctx.beginPath()
        ctx.arc(cx, cy, radius * 3.1, 0, Math.PI * 2)
        ctx.fill()

        ctx.strokeStyle = this.toRgba(coreB, 0.34)
        ctx.lineWidth = 1.2
        ctx.beginPath()
        ctx.arc(cx, cy, radius * 1.8, 0, Math.PI * 2)
        ctx.stroke()
    }

    private ensureParticleCount(count: number): void {
        const target = this.clamp(Math.round(count), 16, 320)
        if (target === this.particleCount && this.particles.length === target) return

        if (target > this.particles.length) {
            for (let i = this.particles.length; i < target; i++) {
                this.particles.push(this.createSeed(i))
            }
        } else {
            this.particles.length = target
        }

        this.particleCount = target
    }

    private createSeed(index: number): ParticleSeed {
        const i = index + 1
        return {
            orbitOffset: this.hash(i * 1.137 + 0.29),
            radialOffset: this.hash(i * 2.413 + 3.18),
            speedBias: this.hash(i * 3.977 + 8.24),
            sizeBias: this.hash(i * 5.331 + 1.77),
            jitter: this.hash(i * 7.739 + 5.65),
            twinkle: this.hash(i * 11.129 + 9.92),
        }
    }

    private resolveDirection(mode: string, arm: number, time: number): number {
        switch (mode) {
            case 'Clockwise':
                return 1
            case 'Counter-Clockwise':
                return -1
            case 'Pulse':
                return Math.sin(time * 1.25 + arm * 0.75) >= 0 ? 1 : -1
            case 'Alternating':
            default:
                return arm % 2 === 0 ? 1 : -1
        }
    }

    private resolveColor(
        mode: string,
        arm: number,
        index: number,
        arms: number,
        time: number,
        life: number,
        cycleRate: number,
    ): RGB {
        const rgbArmPalette: RGB[] = [
            { r: 255, g: 74, b: 86 },
            { r: 78, g: 255, b: 155 },
            { r: 82, g: 146, b: 255 },
        ]

        if (mode === 'RGB Split') {
            const base = rgbArmPalette[arm % rgbArmPalette.length]
            const boost = 0.78 + 0.22 * Math.sin(time * (2.6 + cycleRate * 4.2) + life * Math.PI * 2)
            return this.scaleColor(base, boost)
        }

        if (mode === 'RGB Cycle') {
            const hue = (time * (52 + cycleRate * 188) + (arm / Math.max(arms, 1)) * 360 + life * 90) % 360
            return this.hslToRgb(hue, 92, 60)
        }

        if (mode === 'Prism') {
            const hue = (index * 137.508 + time * (26 + cycleRate * 96) + life * 122) % 360
            return this.hslToRgb(hue, 88, 58)
        }

        const monoHue = 208 + Math.sin(time * (2.2 + cycleRate * 3.8) + arm) * 22
        return this.hslToRgb(monoHue, 48, 72)
    }

    private scaleColor(color: RGB, scale: number): RGB {
        return {
            r: this.clamp(Math.round(color.r * scale), 0, 255),
            g: this.clamp(Math.round(color.g * scale), 0, 255),
            b: this.clamp(Math.round(color.b * scale), 0, 255),
        }
    }

    private toRgba(color: RGB, alpha: number): string {
        return `rgba(${color.r},${color.g},${color.b},${this.clamp(alpha, 0, 1).toFixed(3)})`
    }

    private hslToRgb(h: number, s: number, l: number): RGB {
        const hue = ((h % 360) + 360) % 360
        const sat = this.clamp(s / 100, 0, 1)
        const light = this.clamp(l / 100, 0, 1)
        const c = (1 - Math.abs(2 * light - 1)) * sat
        const x = c * (1 - Math.abs(((hue / 60) % 2) - 1))
        const m = light - c / 2

        let r = 0
        let g = 0
        let b = 0

        if (hue < 60) {
            r = c
            g = x
        } else if (hue < 120) {
            r = x
            g = c
        } else if (hue < 180) {
            g = c
            b = x
        } else if (hue < 240) {
            g = x
            b = c
        } else if (hue < 300) {
            r = x
            b = c
        } else {
            r = c
            b = x
        }

        return {
            r: Math.round((r + m) * 255),
            g: Math.round((g + m) * 255),
            b: Math.round((b + m) * 255),
        }
    }

    private normalizeHexColor(value: string, fallback: string): string {
        const input = value.trim().replace(/^#/, '')
        const fallbackInput = fallback.trim().replace(/^#/, '')

        if (/^[0-9a-fA-F]{6}$/.test(input)) return `#${input.toLowerCase()}`
        if (/^[0-9a-fA-F]{3}$/.test(input)) {
            const expanded = input
                .split('')
                .map((part) => `${part}${part}`)
                .join('')
            return `#${expanded.toLowerCase()}`
        }
        if (/^[0-9a-fA-F]{6}$/.test(fallbackInput)) return `#${fallbackInput.toLowerCase()}`
        return DEFAULT_BACKGROUND
    }

    private pickValue(value: string, options: string[], fallback: string): string {
        return options.includes(value) ? value : fallback
    }

    private clamp(value: number, min: number, max: number): number {
        return Math.max(min, Math.min(max, value))
    }

    private fract(value: number): number {
        return value - Math.floor(value)
    }

    private hash(value: number): number {
        const x = Math.sin(value * 127.1) * 43758.5453123
        return x - Math.floor(x)
    }
}

const effect = new VoronoiGlass()
initializeEffect(() => effect.initialize(), { instance: effect })
