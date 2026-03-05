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

interface FlowControls {
    scene: string
    colorMode: string
    baseColor: string
    count: number
    size: number
    speed: number
    wander: number
    glow: number
    bgColor: string
}

interface TrailPoint {
    x: number
    y: number
}

interface Firefly {
    x: number
    y: number
    vx: number
    vy: number
    phase: number
    hueOffset: number
    satOffset: number
    lightOffset: number
    sizeJitter: number
    speedBias: number
    wanderBias: number
    trail: TrailPoint[]
}

interface RGB {
    r: number
    g: number
    b: number
}

interface HSL {
    h: number
    s: number
    l: number
}

interface SceneTuning {
    speed: number
    wander: number
    swirl: number
    cohesion: number
    pulse: number
    trail: number
    sparkle: number
}

const SCENES = ['Calm', 'Swarm', 'Pulse']
const COLOR_MODES = ['Single', 'Random', 'Rainbow']

const SCENE_TUNING: Record<string, SceneTuning> = {
    Calm: {
        speed: 0.72,
        wander: 0.55,
        swirl: 0.35,
        cohesion: 0.072,
        pulse: 0.2,
        trail: 10,
        sparkle: 0.7,
    },
    Swarm: {
        speed: 1.12,
        wander: 1.12,
        swirl: 0.8,
        cohesion: 0.096,
        pulse: 0.35,
        trail: 7,
        sparkle: 1.0,
    },
    Pulse: {
        speed: 0.9,
        wander: 0.74,
        swirl: 0.58,
        cohesion: 0.082,
        pulse: 1.0,
        trail: 9,
        sparkle: 1.22,
    },
}

@Effect({
    name: 'Flow Field',
    description: 'Poison-glow firefly garden with crisp trails and scene moods',
    author: 'Hypercolor',
    audioReactive: false,
})
class FlowField extends CanvasEffect<FlowControls> {
    @ComboboxControl({ label: 'Scene', values: SCENES, default: 'Calm', tooltip: 'Movement behavior preset' })
    scene!: string

    @ComboboxControl({
        label: 'Color Mode',
        values: COLOR_MODES,
        default: 'Single',
        tooltip: 'Single hue, random fireflies, or animated rainbow',
    })
    colorMode!: string

    @ColorControl({ label: 'Base Color', default: '#b8ff4f', tooltip: 'Primary firefly color' })
    baseColor!: string

    @NumberControl({ label: 'Count', min: 8, max: 220, default: 88, tooltip: 'Number of fireflies' })
    count!: number

    @NumberControl({ label: 'Size', min: 1, max: 10, default: 3, tooltip: 'Firefly body size' })
    size!: number

    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Swarm movement speed' })
    speed!: number

    @NumberControl({ label: 'Wander', min: 0, max: 100, default: 62, tooltip: 'Random drift amount' })
    wander!: number

    @NumberControl({ label: 'Glow', min: 0, max: 100, default: 72, tooltip: 'Glow and trail intensity' })
    glow!: number

    @ColorControl({ label: 'Background Color', default: '#05060a', tooltip: 'Garden background color' })
    bgColor!: string

    private controlState: FlowControls = {
        scene: 'Calm',
        colorMode: 'Single',
        baseColor: '#b8ff4f',
        count: 88,
        size: 3,
        speed: normalizeSpeed(5),
        wander: 62,
        glow: 72,
        bgColor: '#05060a',
    }

    private fireflies: Firefly[] = []
    private cachedBaseColor = '#b8ff4f'
    private cachedBaseHsl: HSL = { h: 90, s: 0.9, l: 0.64 }

    constructor() {
        super({ id: 'flow-field', name: 'Flow Field', backgroundColor: '#05060a' })
    }

    protected initializeControls(): void {
        this.scene = getControlValue('scene', 'Calm')
        this.colorMode = getControlValue('colorMode', 'Single')
        this.baseColor = getControlValue('baseColor', '#b8ff4f')
        this.count = getControlValue('count', 88)
        this.size = getControlValue('size', 3)
        this.speed = getControlValue('speed', 5)
        this.wander = getControlValue('wander', 62)
        this.glow = getControlValue('glow', 72)
        this.bgColor = getControlValue('bgColor', '#05060a')
    }

    protected getControlValues(): FlowControls {
        return {
            scene: getControlValue('scene', 'Calm'),
            colorMode: getControlValue('colorMode', 'Single'),
            baseColor: getControlValue('baseColor', '#b8ff4f'),
            count: getControlValue('count', 88),
            size: getControlValue('size', 3),
            speed: normalizeSpeed(getControlValue('speed', 5)),
            wander: getControlValue('wander', 62),
            glow: getControlValue('glow', 72),
            bgColor: getControlValue('bgColor', '#05060a'),
        }
    }

    protected applyControls(controls: FlowControls): void {
        this.controlState = controls
        this.backgroundColor = controls.bgColor
        this.updateBaseColorCache(controls.baseColor)
        this.ensureFireflyCount(Math.floor(controls.count))
    }

    protected async loadResources(): Promise<void> {
        this.updateBaseColorCache(this.controlState.baseColor)
        this.ensureFireflyCount(Math.floor(this.controlState.count))
    }

    protected draw(time: number, deltaTime: number): void {
        if (!this.ctx || !this.canvas) return

        const ctx = this.ctx
        const w = this.canvas.width
        const h = this.canvas.height
        const dt = deltaTime > 0 ? this.clamp(deltaTime * 60, 0.55, 2.4) : 1

        const scene = SCENE_TUNING[this.controlState.scene] ?? SCENE_TUNING.Calm
        const glowMix = this.clamp(this.controlState.glow / 100, 0, 1)
        const wanderMix = this.clamp(this.controlState.wander / 100, 0, 1)
        const baseSize = this.clamp(this.controlState.size, 1, 10)

        this.drawAtmosphere(ctx, w, h, time, scene, glowMix)

        ctx.save()
        ctx.globalCompositeOperation = 'lighter'
        ctx.lineCap = 'round'
        ctx.lineJoin = 'round'

        const particleCount = this.fireflies.length

        for (let i = 0; i < particleCount; i++) {
            const firefly = this.fireflies[i]
            this.updateFirefly(firefly, w, h, time, dt, scene, wanderMix, glowMix)

            const twinkle = this.getTwinkle(firefly, time, scene)
            const color = this.resolveColor(firefly, i, time, twinkle, particleCount)
            const radius = Math.max(0.8, baseSize * (0.42 + twinkle * 0.58) * firefly.sizeJitter)

            this.drawTrail(ctx, firefly, color, radius, glowMix, scene, twinkle)
            this.drawBody(ctx, firefly, color, radius, glowMix, twinkle)
        }

        ctx.restore()
    }

    private drawAtmosphere(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        time: number,
        scene: SceneTuning,
        glowMix: number,
    ): void {
        const hueShift = Math.sin(time * 0.08) * 6
        const mistColor = this.hslToRgb((this.cachedBaseHsl.h + 12 + hueShift + 360) % 360, 82, 48)

        const mist = ctx.createRadialGradient(
            w * (0.5 + Math.sin(time * 0.04) * 0.08),
            h * (0.55 + Math.cos(time * 0.05) * 0.08),
            0,
            w * 0.5,
            h * 0.5,
            Math.max(w, h) * 0.9,
        )
        mist.addColorStop(0, this.rgba(mistColor, 0.08 + glowMix * 0.18))
        mist.addColorStop(0.58, this.rgba(mistColor, 0.03 + glowMix * 0.06))
        mist.addColorStop(1, this.rgba(mistColor, 0))
        ctx.fillStyle = mist
        ctx.fillRect(0, 0, w, h)

        const pulse = 0.035 + scene.pulse * 0.018 * (0.5 + 0.5 * Math.sin(time * 2.2))
        const veil = this.hslToRgb((this.cachedBaseHsl.h + 300) % 360, 70, 38)
        const gradient = ctx.createLinearGradient(0, 0, 0, h)
        gradient.addColorStop(0, this.rgba(veil, pulse))
        gradient.addColorStop(1, this.rgba(veil, 0))
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, w, h)

        const vignette = ctx.createRadialGradient(w * 0.5, h * 0.5, 20, w * 0.5, h * 0.5, Math.max(w, h) * 0.8)
        vignette.addColorStop(0, 'rgba(0, 0, 0, 0)')
        vignette.addColorStop(1, 'rgba(0, 0, 0, 0.42)')
        ctx.fillStyle = vignette
        ctx.fillRect(0, 0, w, h)
    }

    private updateFirefly(
        firefly: Firefly,
        w: number,
        h: number,
        time: number,
        dt: number,
        scene: SceneTuning,
        wanderMix: number,
        glowMix: number,
    ): void {
        const centerX = w * 0.5 + Math.sin(time * 0.24) * w * 0.2
        const centerY = h * 0.5 + Math.cos(time * 0.19) * h * 0.16

        const dx = centerX - firefly.x
        const dy = centerY - firefly.y
        const distance = Math.max(1, Math.hypot(dx, dy))

        const dirX = dx / distance
        const dirY = dy / distance
        const swirlX = -dirY
        const swirlY = dirX

        const jitterA = Math.sin(time * 1.7 + firefly.phase * 8.3 + firefly.x * 0.03)
        const jitterB = Math.cos(time * 1.33 + firefly.phase * 6.1 + firefly.y * 0.027)
        const wanderForce = (0.02 + wanderMix * 0.12) * scene.wander * firefly.wanderBias

        firefly.vx += (jitterA * 0.8 + jitterB * 0.5) * wanderForce * dt
        firefly.vy += (jitterB * 0.9 - jitterA * 0.45) * wanderForce * dt

        const centerPull = scene.cohesion * (0.85 + (distance / Math.max(w, h)) * 0.3)
        firefly.vx += dirX * centerPull * dt
        firefly.vy += dirY * centerPull * dt

        const swirlForce = scene.swirl * (0.035 + glowMix * 0.02)
        firefly.vx += swirlX * swirlForce * dt
        firefly.vy += swirlY * swirlForce * dt

        if (this.controlState.scene === 'Pulse') {
            const pulse = Math.sin(time * 4.1 + firefly.phase * 7.2)
            const pulseForce = scene.pulse * (0.11 + firefly.speedBias * 0.05)
            firefly.vx += dirX * pulse * pulseForce * dt
            firefly.vy += dirY * pulse * pulseForce * dt
        }

        firefly.vx *= 0.93
        firefly.vy *= 0.93

        const speedScale = this.controlState.speed * scene.speed * (0.55 + firefly.speedBias * 0.6)
        const maxVelocity = Math.max(0.75, speedScale * 1.5)
        const velocity = Math.hypot(firefly.vx, firefly.vy)

        if (velocity > maxVelocity) {
            const scale = maxVelocity / velocity
            firefly.vx *= scale
            firefly.vy *= scale
        }

        firefly.x += firefly.vx * speedScale * dt
        firefly.y += firefly.vy * speedScale * dt

        const wrapped = this.wrapFirefly(firefly, w, h)
        if (wrapped) {
            firefly.trail = [{ x: firefly.x, y: firefly.y }]
            return
        }

        firefly.trail.unshift({ x: firefly.x, y: firefly.y })
        const trailLength = Math.max(6, Math.floor(scene.trail + glowMix * 5))
        if (firefly.trail.length > trailLength) {
            firefly.trail.length = trailLength
        }
    }

    private drawTrail(
        ctx: CanvasRenderingContext2D,
        firefly: Firefly,
        color: RGB,
        radius: number,
        glowMix: number,
        scene: SceneTuning,
        twinkle: number,
    ): void {
        const trail = firefly.trail
        if (trail.length < 2) return

        for (let i = 1; i < trail.length; i++) {
            const head = trail[i - 1]
            const tail = trail[i]
            const depth = 1 - i / trail.length
            const alpha = depth * depth * (0.07 + glowMix * 0.2) * (0.65 + twinkle * 0.7)
            const width = Math.max(0.45, radius * (0.28 + depth * (0.9 + scene.pulse * 0.16)))

            ctx.strokeStyle = this.rgba(color, alpha)
            ctx.lineWidth = width
            ctx.beginPath()
            ctx.moveTo(head.x, head.y)
            ctx.lineTo(tail.x, tail.y)
            ctx.stroke()
        }
    }

    private drawBody(
        ctx: CanvasRenderingContext2D,
        firefly: Firefly,
        color: RGB,
        radius: number,
        glowMix: number,
        twinkle: number,
    ): void {
        const haloRadius = radius * (1.7 + glowMix * 4.3)
        if (glowMix > 0.01) {
            const glow = ctx.createRadialGradient(firefly.x, firefly.y, 0, firefly.x, firefly.y, haloRadius)
            glow.addColorStop(0, this.rgba(color, (0.2 + glowMix * 0.4) * (0.55 + twinkle * 0.75)))
            glow.addColorStop(0.6, this.rgba(color, 0.08 + glowMix * 0.12))
            glow.addColorStop(1, this.rgba(color, 0))
            ctx.fillStyle = glow
            ctx.beginPath()
            ctx.arc(firefly.x, firefly.y, haloRadius, 0, Math.PI * 2)
            ctx.fill()
        }

        ctx.fillStyle = this.rgba(color, 0.82 + twinkle * 0.18)
        ctx.beginPath()
        ctx.arc(firefly.x, firefly.y, radius, 0, Math.PI * 2)
        ctx.fill()

        ctx.fillStyle = `rgba(255, 255, 236, ${Math.min(1, 0.24 + twinkle * 0.55).toFixed(3)})`
        ctx.beginPath()
        ctx.arc(firefly.x - radius * 0.24, firefly.y - radius * 0.28, radius * 0.38, 0, Math.PI * 2)
        ctx.fill()
    }

    private resolveColor(firefly: Firefly, index: number, time: number, twinkle: number, count: number): RGB {
        const mode = this.controlState.colorMode
        const base = this.cachedBaseHsl

        if (mode === 'Random') {
            const hue = (base.h + firefly.hueOffset * 260 + Math.sin(time * 0.24 + firefly.phase * 5.3) * 22 + 360) % 360
            const sat = this.clamp(72 + firefly.satOffset * 22, 48, 100)
            const light = this.clamp(43 + twinkle * 22 + firefly.lightOffset * 9, 30, 88)
            return this.hslToRgb(hue, sat, light)
        }

        if (mode === 'Rainbow') {
            const hue = (time * 36 + index * (360 / Math.max(count, 1)) + firefly.hueOffset * 40 + 360) % 360
            const sat = this.clamp(80 + firefly.satOffset * 16, 58, 100)
            const light = this.clamp(48 + twinkle * 20 + firefly.lightOffset * 6, 34, 90)
            return this.hslToRgb(hue, sat, light)
        }

        const hue = (base.h + firefly.hueOffset * 20 + Math.sin(time * 0.4 + firefly.phase * 3.7) * 8 + 360) % 360
        const sat = this.clamp(base.s * 100 + 10 + firefly.satOffset * 6, 42, 100)
        const light = this.clamp(base.l * 100 + twinkle * 16 + 4, 24, 88)
        return this.hslToRgb(hue, sat, light)
    }

    private getTwinkle(firefly: Firefly, time: number, scene: SceneTuning): number {
        const pulse = Math.sin(time * (1.4 + scene.sparkle) + firefly.phase * 1.9)
        const shimmer = Math.sin(time * 3.4 + firefly.phase * 6.7) * 0.5
        return this.clamp(0.54 + pulse * 0.3 + shimmer * 0.2, 0.12, 1)
    }

    private ensureFireflyCount(count: number): void {
        const target = Math.max(8, Math.min(220, count))
        const w = this.canvas?.width ?? 320
        const h = this.canvas?.height ?? 200

        if (this.fireflies.length < target) {
            while (this.fireflies.length < target) {
                this.fireflies.push(this.createFirefly(w, h))
            }
            return
        }

        if (this.fireflies.length > target) {
            this.fireflies.length = target
        }
    }

    private createFirefly(w: number, h: number): Firefly {
        const x = Math.random() * w
        const y = Math.random() * h
        return {
            x,
            y,
            vx: (Math.random() - 0.5) * 0.8,
            vy: (Math.random() - 0.5) * 0.8,
            phase: Math.random() * Math.PI * 2,
            hueOffset: Math.random() * 2 - 1,
            satOffset: Math.random() * 2 - 1,
            lightOffset: Math.random() * 2 - 1,
            sizeJitter: 0.68 + Math.random() * 0.72,
            speedBias: 0.75 + Math.random() * 0.55,
            wanderBias: 0.8 + Math.random() * 0.45,
            trail: [{ x, y }],
        }
    }

    private wrapFirefly(firefly: Firefly, w: number, h: number): boolean {
        const margin = 12
        let wrapped = false

        if (firefly.x < -margin) {
            firefly.x = w + margin
            wrapped = true
        } else if (firefly.x > w + margin) {
            firefly.x = -margin
            wrapped = true
        }

        if (firefly.y < -margin) {
            firefly.y = h + margin
            wrapped = true
        } else if (firefly.y > h + margin) {
            firefly.y = -margin
            wrapped = true
        }

        return wrapped
    }

    private updateBaseColorCache(color: string): void {
        if (color === this.cachedBaseColor) return
        const rgb = this.hexToRgb(color)
        this.cachedBaseColor = color
        this.cachedBaseHsl = this.rgbToHsl(rgb)
    }

    private rgba(color: RGB, alpha: number): string {
        return `rgba(${color.r}, ${color.g}, ${color.b}, ${this.clamp(alpha, 0, 1).toFixed(3)})`
    }

    private clamp(value: number, min: number, max: number): number {
        if (Number.isNaN(value)) return min
        return Math.max(min, Math.min(max, value))
    }

    private hexToRgb(hex: string): RGB {
        const normalized = hex.replace('#', '')
        const full = normalized.length === 3
            ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
            : normalized

        const parsed = parseInt(full, 16)
        if (Number.isNaN(parsed)) {
            return { r: 184, g: 255, b: 79 }
        }

        return {
            r: (parsed >> 16) & 255,
            g: (parsed >> 8) & 255,
            b: parsed & 255,
        }
    }

    private rgbToHsl(color: RGB): HSL {
        const r = color.r / 255
        const g = color.g / 255
        const b = color.b / 255

        const max = Math.max(r, g, b)
        const min = Math.min(r, g, b)
        const delta = max - min

        const l = (max + min) / 2

        if (delta === 0) {
            return { h: 0, s: 0, l }
        }

        const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)

        let h: number
        if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
        else if (max === g) h = (b - r) / delta + 2
        else h = (r - g) / delta + 4

        h *= 60

        return { h, s, l }
    }

    private hslToRgb(h: number, sPercent: number, lPercent: number): RGB {
        const s = this.clamp(sPercent, 0, 100) / 100
        const l = this.clamp(lPercent, 0, 100) / 100
        const c = (1 - Math.abs(2 * l - 1)) * s
        const hPrime = ((h % 360) + 360) % 360 / 60
        const x = c * (1 - Math.abs((hPrime % 2) - 1))

        let r = 0
        let g = 0
        let b = 0

        if (hPrime < 1) [r, g, b] = [c, x, 0]
        else if (hPrime < 2) [r, g, b] = [x, c, 0]
        else if (hPrime < 3) [r, g, b] = [0, c, x]
        else if (hPrime < 4) [r, g, b] = [0, x, c]
        else if (hPrime < 5) [r, g, b] = [x, 0, c]
        else [r, g, b] = [c, 0, x]

        const m = l - c / 2

        return {
            r: Math.round((r + m) * 255),
            g: Math.round((g + m) * 255),
            b: Math.round((b + m) * 255),
        }
    }
}

const effect = new FlowField()
initializeEffect(() => effect.initialize(), { instance: effect })
