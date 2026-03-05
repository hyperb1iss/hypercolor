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

interface MeteorControls {
    path: string
    speed: number
    starSize: number
    density: number
    trail: number
    skyTop: string
    skyBottom: string
    starColor: string
    scene: string
}

interface Star {
    x: number
    y: number
    size: number
    phase: number
    depth: number
}

interface Meteor {
    x: number
    y: number
    vx: number
    vy: number
    size: number
    trail: number
    phase: number
    brightness: number
}

interface Rgb {
    r: number
    g: number
    b: number
}

interface SceneTone {
    skyTop: Rgb
    skyBottom: Rgb
    star: Rgb
    trail: Rgb
}

const PATHS = ['Diagonal', 'Vertical']
const SCENES = ['Night', 'Pastel']

@Effect({
    name: 'Meteor Storm',
    description: 'Crisp falling stars with directional hyperspace trails over a gradient night sky',
    author: 'Hypercolor',
    audioReactive: false,
})
class MeteorStorm extends CanvasEffect<MeteorControls> {
    @ComboboxControl({ label: 'Path', values: PATHS, default: 'Diagonal', tooltip: 'Meteor trajectory direction' })
    path!: string

    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Meteor travel speed' })
    speed!: number

    @NumberControl({ label: 'Star Size', min: 1, max: 20, default: 8, tooltip: 'Meteor head and star size' })
    starSize!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 58, tooltip: 'Star and meteor count' })
    density!: number

    @NumberControl({ label: 'Trail', min: 10, max: 100, default: 72, tooltip: 'Meteor trail length and glow' })
    trail!: number

    @ColorControl({ label: 'Sky Top color', default: '#0b1233', tooltip: 'Top gradient color' })
    skyTop!: string

    @ColorControl({ label: 'Sky Bottom color', default: '#1a3f89', tooltip: 'Bottom gradient color' })
    skyBottom!: string

    @ColorControl({ label: 'Star Color', default: '#fff6bd', tooltip: 'Star and meteor color' })
    starColor!: string

    @ComboboxControl({ label: 'Scene', values: SCENES, default: 'Night', tooltip: 'Scene tint preset' })
    scene!: string

    private controlState: MeteorControls = {
        path: 'Diagonal',
        speed: normalizeSpeed(5),
        starSize: 8,
        density: 58,
        trail: 72,
        skyTop: '#0b1233',
        skyBottom: '#1a3f89',
        starColor: '#fff6bd',
        scene: 'Night',
    }

    private stars: Star[] = []
    private meteors: Meteor[] = []
    private spawnBudget = 0
    private lastWidth = 0
    private lastHeight = 0
    private lastPath = 'Diagonal'
    private lastStarCount = 0

    constructor() {
        super({
            id: 'meteor-storm',
            name: 'Meteor Storm',
            backgroundColor: '#060a17',
        })
    }

    protected initializeControls(): void {
        this.path = getControlValue('path', 'Diagonal')
        this.speed = getControlValue('speed', 5)
        this.starSize = getControlValue('starSize', 8)
        this.density = getControlValue('density', 58)
        this.trail = getControlValue('trail', 72)
        this.skyTop = getControlValue('skyTop', '#0b1233')
        this.skyBottom = getControlValue('skyBottom', '#1a3f89')
        this.starColor = getControlValue('starColor', '#fff6bd')
        this.scene = getControlValue('scene', 'Night')
    }

    protected getControlValues(): MeteorControls {
        return {
            path: getControlValue('path', 'Diagonal'),
            speed: normalizeSpeed(getControlValue('speed', 5)),
            starSize: getControlValue('starSize', 8),
            density: getControlValue('density', 58),
            trail: getControlValue('trail', 72),
            skyTop: getControlValue('skyTop', '#0b1233'),
            skyBottom: getControlValue('skyBottom', '#1a3f89'),
            starColor: getControlValue('starColor', '#fff6bd'),
            scene: getControlValue('scene', 'Night'),
        }
    }

    protected async loadResources(): Promise<void> {
        if (!this.canvas) return
        this.syncStarfield(this.canvas.width, this.canvas.height, true)
    }

    protected applyControls(controls: MeteorControls): void {
        if (controls.path !== this.lastPath) {
            this.meteors = []
            this.spawnBudget = 0
            this.lastPath = controls.path
        }
        this.controlState = controls
        this.backgroundColor = controls.skyBottom
    }

    protected draw(time: number, deltaTime: number): void {
        if (!this.ctx || !this.canvas) return

        const ctx = this.ctx
        const w = this.canvas.width
        const h = this.canvas.height
        const dt = deltaTime > 0 ? Math.min(0.05, deltaTime) : 1 / 60

        this.syncStarfield(w, h)

        const tone = this.getSceneTone(this.controlState)
        this.drawSky(ctx, w, h, tone)
        this.drawBackgroundStars(ctx, w, h, time, tone)
        this.updateMeteors(dt, w, h)
        this.drawMeteors(ctx, time, tone)
    }

    private syncStarfield(w: number, h: number, force = false): void {
        const targetCount = this.computeBackgroundStarCount()
        const sizeChanged = this.lastWidth !== w || this.lastHeight !== h
        if (!force && !sizeChanged && targetCount === this.lastStarCount) return

        this.lastWidth = w
        this.lastHeight = h
        this.lastStarCount = targetCount
        this.stars = []

        for (let i = 0; i < targetCount; i++) {
            this.stars.push({
                x: Math.random() * w,
                y: Math.random() * h,
                size: 0.35 + Math.random() * 1.4,
                phase: Math.random() * Math.PI * 2,
                depth: 0.25 + Math.random() * 0.95,
            })
        }
    }

    private drawSky(ctx: CanvasRenderingContext2D, w: number, h: number, tone: SceneTone): void {
        const gradient = ctx.createLinearGradient(0, 0, 0, h)
        gradient.addColorStop(0, this.rgbToCss(tone.skyTop))
        gradient.addColorStop(1, this.rgbToCss(tone.skyBottom))
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, w, h)

        const haze = ctx.createLinearGradient(0, h * 0.35, 0, h)
        haze.addColorStop(0, this.rgbToRgba(tone.trail, 0))
        haze.addColorStop(1, this.rgbToRgba(tone.trail, 0.04 + this.controlState.trail * 0.0012))
        ctx.fillStyle = haze
        ctx.fillRect(0, 0, w, h)
    }

    private drawBackgroundStars(ctx: CanvasRenderingContext2D, w: number, h: number, time: number, tone: SceneTone): void {
        const drift = 8 + this.controlState.speed * 18
        const driftX = this.controlState.path === 'Diagonal' ? drift * 0.38 : drift * 0.04
        const driftY = this.controlState.path === 'Vertical' ? drift * 0.52 : drift * 0.34
        const sizeScale = 0.6 + this.controlState.starSize * 0.06

        for (const star of this.stars) {
            const x = (star.x + time * driftX * star.depth) % w
            const y = (star.y + time * driftY * (0.4 + star.depth)) % h
            const twinkle = 0.5 + 0.5 * Math.sin(time * (1.2 + star.depth * 1.6) + star.phase * 2.4)
            const alpha = (0.15 + twinkle * 0.46) * (0.72 + star.depth * 0.28)
            const size = Math.max(1, (star.size + sizeScale * 0.22) * (0.8 + star.depth * 0.28))

            ctx.fillStyle = this.rgbToRgba(tone.star, alpha * 0.7)
            ctx.fillRect(x, y, size, size)

            if (size > 1.45 && twinkle > 0.76) {
                ctx.fillStyle = this.rgbToRgba(tone.star, alpha * 0.5)
                const streak = size * 2.2
                ctx.fillRect(x - streak * 0.5, y + size * 0.1, streak, 1)
                ctx.fillRect(x + size * 0.1, y - streak * 0.5, 1, streak)
            }
        }
    }

    private updateMeteors(dt: number, w: number, h: number): void {
        const maxMeteors = this.computeMeteorCap()
        const spawnRate = this.computeMeteorSpawnRate()

        this.spawnBudget += dt * spawnRate
        while (this.spawnBudget >= 1 && this.meteors.length < maxMeteors) {
            this.meteors.push(this.spawnMeteor(w, h))
            this.spawnBudget -= 1
        }

        if (this.meteors.length < maxMeteors && Math.random() < dt * spawnRate * 0.4) {
            this.meteors.push(this.spawnMeteor(w, h))
        }

        for (const meteor of this.meteors) {
            meteor.x += meteor.vx * dt
            meteor.y += meteor.vy * dt
            meteor.brightness = 0.72 + 0.28 * Math.sin(meteor.phase + meteor.y * 0.03)
        }

        this.meteors = this.meteors.filter(
            (meteor) =>
                meteor.x > -meteor.trail - 45 &&
                meteor.x < w + meteor.trail + 45 &&
                meteor.y < h + meteor.trail + 55,
        )
    }

    private spawnMeteor(w: number, h: number): Meteor {
        const baseSpeed = 65 + this.controlState.speed * 120
        const size = Math.max(1.2, 0.65 + this.controlState.starSize * 0.16) * (0.72 + Math.random() * 0.65)
        const trail = (12 + this.controlState.trail * 1.45 + size * 4.5) * (0.82 + Math.random() * 0.36)

        if (this.controlState.path === 'Vertical') {
            const vy = baseSpeed * (0.85 + Math.random() * 0.8)
            const vx = (Math.random() - 0.5) * baseSpeed * 0.09
            return {
                x: Math.random() * (w + 24) - 12,
                y: -trail - Math.random() * h * 0.32,
                vx,
                vy,
                size,
                trail,
                phase: Math.random() * Math.PI * 2,
                brightness: 1,
            }
        }

        const vy = baseSpeed * (0.78 + Math.random() * 0.76)
        const direction = Math.random() < 0.86 ? 1 : -1
        const vx = baseSpeed * (0.3 + Math.random() * 0.34) * direction
        return {
            x: Math.random() * (w + 32) - 16,
            y: -trail - Math.random() * h * 0.42,
            vx,
            vy,
            size,
            trail,
            phase: Math.random() * Math.PI * 2,
            brightness: 1,
        }
    }

    private drawMeteors(ctx: CanvasRenderingContext2D, time: number, tone: SceneTone): void {
        const headColor = this.mixRgb(tone.star, { r: 255, g: 255, b: 255 }, 0.35)

        for (const meteor of this.meteors) {
            const velocity = Math.hypot(meteor.vx, meteor.vy)
            if (velocity <= 0.0001) continue

            const ux = meteor.vx / velocity
            const uy = meteor.vy / velocity
            const dynamicTrail = meteor.trail * (0.84 + 0.16 * Math.sin(time * 4.6 + meteor.phase))
            const tailX = meteor.x - ux * dynamicTrail
            const tailY = meteor.y - uy * dynamicTrail

            const mainTrail = ctx.createLinearGradient(meteor.x, meteor.y, tailX, tailY)
            mainTrail.addColorStop(0, this.rgbToRgba(headColor, 0.92 * meteor.brightness))
            mainTrail.addColorStop(0.24, this.rgbToRgba(tone.star, 0.58 * meteor.brightness))
            mainTrail.addColorStop(1, this.rgbToRgba(tone.trail, 0))

            ctx.lineCap = 'round'
            ctx.lineWidth = Math.max(1.15, meteor.size * 0.82)
            ctx.strokeStyle = mainTrail
            ctx.beginPath()
            ctx.moveTo(meteor.x, meteor.y)
            ctx.lineTo(tailX, tailY)
            ctx.stroke()

            const bloomTrail = ctx.createLinearGradient(meteor.x, meteor.y, tailX, tailY)
            bloomTrail.addColorStop(0, this.rgbToRgba(tone.trail, 0.28 * meteor.brightness))
            bloomTrail.addColorStop(1, this.rgbToRgba(tone.trail, 0))
            ctx.lineWidth = Math.max(2.25, meteor.size * 2.05)
            ctx.strokeStyle = bloomTrail
            ctx.beginPath()
            ctx.moveTo(meteor.x, meteor.y)
            ctx.lineTo(tailX, tailY)
            ctx.stroke()

            const headSize = Math.max(1.5, meteor.size * 0.92)
            ctx.fillStyle = this.rgbToRgba(headColor, 0.98)
            ctx.fillRect(meteor.x - headSize * 0.5, meteor.y - headSize * 0.5, headSize, headSize)

            const flare = headSize * 1.85
            ctx.fillStyle = this.rgbToRgba(headColor, 0.56 * meteor.brightness)
            ctx.fillRect(meteor.x - flare * 0.5, meteor.y - 0.5, flare, 1)
            ctx.fillRect(meteor.x - 0.5, meteor.y - flare * 0.5, 1, flare)
        }
    }

    private computeBackgroundStarCount(): number {
        return Math.max(36, Math.floor(40 + this.controlState.density * 1.9 - this.controlState.starSize * 0.55))
    }

    private computeMeteorCap(): number {
        return Math.max(4, Math.floor(3 + this.controlState.density * 0.15 - this.controlState.starSize * 0.05))
    }

    private computeMeteorSpawnRate(): number {
        return 1 + this.controlState.density * 0.05 + this.controlState.speed * 0.8
    }

    private getSceneTone(controls: MeteorControls): SceneTone {
        const top = this.hexToRgb(controls.skyTop)
        const bottom = this.hexToRgb(controls.skyBottom)
        const star = this.hexToRgb(controls.starColor)

        if (controls.scene === 'Pastel') {
            return {
                skyTop: this.mixRgb(top, { r: 255, g: 227, b: 246 }, 0.24),
                skyBottom: this.mixRgb(bottom, { r: 210, g: 230, b: 255 }, 0.2),
                star: this.mixRgb(star, { r: 255, g: 255, b: 255 }, 0.24),
                trail: this.mixRgb(star, { r: 255, g: 214, b: 242 }, 0.34),
            }
        }

        return {
            skyTop: this.mixRgb(top, { r: 3, g: 6, b: 18 }, 0.32),
            skyBottom: this.mixRgb(bottom, { r: 15, g: 30, b: 68 }, 0.2),
            star: this.mixRgb(star, { r: 255, g: 255, b: 255 }, 0.16),
            trail: this.mixRgb(star, { r: 146, g: 198, b: 255 }, 0.24),
        }
    }

    private hexToRgb(hex: string): Rgb {
        const normalized = hex.trim().replace('#', '')
        const expanded =
            normalized.length === 3 ? normalized.split('').map((char) => `${char}${char}`).join('') : normalized
        if (!/^[0-9a-fA-F]{6}$/.test(expanded)) {
            return { r: 255, g: 255, b: 255 }
        }
        const value = Number.parseInt(expanded, 16)
        return {
            r: (value >> 16) & 255,
            g: (value >> 8) & 255,
            b: value & 255,
        }
    }

    private mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
        const ratio = Math.max(0, Math.min(1, t))
        return {
            r: Math.round(a.r + (b.r - a.r) * ratio),
            g: Math.round(a.g + (b.g - a.g) * ratio),
            b: Math.round(a.b + (b.b - a.b) * ratio),
        }
    }

    private rgbToCss(color: Rgb): string {
        return `rgb(${color.r}, ${color.g}, ${color.b})`
    }

    private rgbToRgba(color: Rgb, alpha: number): string {
        return `rgba(${color.r}, ${color.g}, ${color.b}, ${Math.max(0, Math.min(1, alpha))})`
    }
}

const effect = new MeteorStorm()
initializeEffect(() => effect.initialize(), { instance: effect })
