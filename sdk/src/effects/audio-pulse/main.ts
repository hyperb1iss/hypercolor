import { audio, canvas, color, combo, num, scaleContext } from 'hypercolor'

import {
    BUILTIN_DESIGN_BASIS,
    clamp01,
    hexToRgb,
    mixRgb,
    rgbToCss,
    scaleRgb,
    seededNoise,
    withLift,
} from '../_builtin/common'

interface PulseWave {
    radius: number
    strength: number
    mix: number
    spin: number
}

function smoothApproach(current: number, target: number, lambda: number, dt: number): number {
    if (!Number.isFinite(lambda) || lambda <= 0) return target
    const factor = 1 - Math.exp(-lambda * Math.max(dt, 0))
    return current + (target - current) * factor
}

const RAY_COUNT = 10
const RAY_SPRITE_WIDTH = 256
const RAY_SPRITE_HEIGHT = 8

/// Bake one ray's gradient strip into an offscreen sprite so the burst is
/// ten cheap rotated drawImage calls instead of ten fresh gradients a frame.
function buildRaySprite(color: string): HTMLCanvasElement | null {
    const sprite = document.createElement('canvas')
    sprite.width = RAY_SPRITE_WIDTH
    sprite.height = RAY_SPRITE_HEIGHT
    const spriteCtx = sprite.getContext('2d')
    if (!spriteCtx) return null
    const gradient = spriteCtx.createLinearGradient(0, 0, RAY_SPRITE_WIDTH, 0)
    gradient.addColorStop(0, 'rgba(255, 255, 255, 0)')
    gradient.addColorStop(0.35, color)
    gradient.addColorStop(1, 'rgba(255, 255, 255, 0)')
    spriteCtx.fillStyle = gradient
    spriteCtx.fillRect(0, 0, RAY_SPRITE_WIDTH, RAY_SPRITE_HEIGHT)
    return sprite
}

function drawRayBurst(
    ctx: CanvasRenderingContext2D,
    sprite: HTMLCanvasElement,
    cx: number,
    cy: number,
    radius: number,
    length: number,
    alpha: number,
    rotation: number,
): void {
    ctx.save()
    ctx.translate(cx, cy)
    ctx.rotate(rotation)
    ctx.globalAlpha = alpha

    const step = (Math.PI * 2) / RAY_COUNT
    for (let i = 0; i < RAY_COUNT; i++) {
        ctx.drawImage(sprite, radius, -3, length, 6)
        ctx.rotate(step)
    }

    ctx.restore()
}

export default canvas.stateful(
    'Audio Pulse',
    {
        style: combo('Style', ['Hybrid', 'Bloom', 'Rings', 'Pulse Rays'], { default: 'Hybrid', group: 'Scene' }),
        baseColor: color('Base Color', '#090f24', { group: 'Colors' }),
        peakColor: color('Peak Color', '#ff3f8f', { group: 'Colors' }),
        accentColor: color('Accent Color', '#80ffea', { group: 'Colors' }),
        sensitivity: num('Sensitivity', [20, 200], 100, { group: 'Audio' }),
        speed: num('Wave Speed', [0, 100], 54, { group: 'Motion', normalize: 'none' }),
        linger: num('Linger', [0, 100], 55, { group: 'Motion' }),
        ringWidth: num('Ring Width', [4, 60], 22, { group: 'Motion' }),
        glow: num('Glow', [0, 100], 58, { group: 'Output' }),
        floor: num('Floor', [0, 100], 20, { group: 'Output' }),
        brightness: num('Brightness', [0, 100], 88, { group: 'Output' }),
    },
    () => {
        let level = 0
        let bass = 0
        let pulse = 0
        let lastTime = 0
        let beatHeld = false
        let waves: PulseWave[] = []
        let raySprite: HTMLCanvasElement | null = null
        let raySpriteKey = ''

        return (ctx, time, controls) => {
            const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
            const width = s.width
            const height = s.height
            const data = audio()
            const dt = Math.min(lastTime > 0 ? time - lastTime : 0.016, 0.05)
            lastTime = time

            const sensitivity = (controls.sensitivity as number) / 100
            const speed = (controls.speed as number) / 100
            const linger = (controls.linger as number) / 100
            const glow = (controls.glow as number) / 100
            const brightness = (controls.brightness as number) / 100
            const floor = (controls.floor as number) / 100

            const fallbackLevel = 0.05 + (0.5 + 0.5 * Math.sin(time * 0.36)) * 0.05
            const fallbackBass = 0.08 + (0.5 + 0.5 * Math.sin(time * 0.84)) * 0.1
            const fallbackPulse = Math.max(0, Math.sin(time * 1.2)) ** 8 * 0.2

            const levelTarget = Math.max(data.levelShort, data.level * 0.88, fallbackLevel)
            const bassTarget = Math.max(data.bassEnv, data.bass * 0.92, fallbackBass)
            const pulseTarget = clamp01(
                Math.max(data.beatPulse, data.onsetPulse * 0.8, data.spectralFlux * 0.32, fallbackPulse),
            )

            level = smoothApproach(level, levelTarget, 6.5, dt)
            bass = smoothApproach(bass, bassTarget, 7.5, dt)
            pulse = smoothApproach(pulse, pulseTarget, pulseTarget > pulse ? 16 : 4.5, dt)

            const gate = pulse * (0.7 + sensitivity * 0.85) > 0.34
            if (gate && !beatHeld) {
                waves.push({
                    mix: seededNoise(time * 17.0 + waves.length * 3.1),
                    radius: 0,
                    spin: seededNoise(time * 11.0 + waves.length * 7.3) * Math.PI * 2,
                    strength: clamp01(pulse * 0.85 + bass * 0.42 + 0.18),
                })
            }
            beatHeld = gate

            const travelPx = s.ds(110 + speed * 180)
            const fadeSeconds = 0.18 + linger * 1.25
            waves = waves
                .map((wave) => ({
                    ...wave,
                    radius: wave.radius + dt * travelPx,
                    strength: wave.strength * Math.exp(-dt / fadeSeconds),
                }))
                .filter((wave) => wave.strength > 0.03 && wave.radius < s.ds(240))

            const base = scaleRgb(hexToRgb(controls.baseColor as string), brightness)
            const peak = scaleRgb(hexToRgb(controls.peakColor as string), brightness)
            const accent = scaleRgb(hexToRgb(controls.accentColor as string), brightness)
            const style = controls.style as string

            const centerX = s.dx(160 + Math.sin(time * (0.22 + speed * 0.28)) * 18 + data.momentum * 8)
            const centerY = s.dy(100 + Math.cos(time * (0.18 + speed * 0.22)) * 12 - data.swell * 10)

            ctx.clearRect(0, 0, width, height)

            const background = ctx.createRadialGradient(centerX, centerY, s.ds(8), centerX, centerY, s.ds(180))
            background.addColorStop(0, rgbToCss(mixRgb(base, peak, clamp01(level * 0.7 + bass * 0.22)), 0.9))
            background.addColorStop(0.45, rgbToCss(mixRgb(base, accent, clamp01(level * 0.32 + floor * 0.4)), 0.82))
            background.addColorStop(1, rgbToCss(scaleRgb(base, 0.28 + floor * 0.45), 1))
            ctx.fillStyle = background
            ctx.fillRect(0, 0, width, height)

            const outerHalo = ctx.createRadialGradient(centerX, centerY, s.ds(16), centerX, centerY, s.ds(130))
            outerHalo.addColorStop(0, rgbToCss(withLift(peak, 0.15), 0.12 + level * 0.1))
            outerHalo.addColorStop(1, rgbToCss(accent, 0))
            ctx.fillStyle = outerHalo
            ctx.fillRect(0, 0, width, height)

            ctx.save()
            ctx.globalCompositeOperation = 'lighter'

            if (style === 'Hybrid' || style === 'Bloom' || style === 'Pulse Rays') {
                const bloom = ctx.createRadialGradient(
                    centerX,
                    centerY,
                    s.ds(6),
                    centerX,
                    centerY,
                    s.ds(46 + glow * 54 + bass * 48),
                )
                bloom.addColorStop(0, rgbToCss(withLift(peak, 0.28), 0.7 + pulse * 0.22))
                bloom.addColorStop(0.35, rgbToCss(mixRgb(peak, accent, 0.42), 0.28 + bass * 0.32))
                bloom.addColorStop(1, rgbToCss(accent, 0))
                ctx.fillStyle = bloom
                ctx.fillRect(0, 0, width, height)
            }

            if (style === 'Hybrid' || style === 'Pulse Rays') {
                const rayColor = rgbToCss(accent, 1)
                if (!raySprite || raySpriteKey !== rayColor) {
                    raySprite = buildRaySprite(rayColor)
                    raySpriteKey = rayColor
                }
                if (raySprite) {
                    // The per-frame ray alpha folds into globalAlpha so the
                    // cached sprite never has to be rebuilt as the beat moves.
                    drawRayBurst(
                        ctx,
                        raySprite,
                        centerX,
                        centerY,
                        s.ds(18),
                        s.ds(42 + glow * 42 + pulse * 18),
                        (0.18 + pulse * 0.14) * (0.4 + pulse * 0.25),
                        time * (0.35 + speed * 0.6),
                    )
                }
            }

            if (style === 'Hybrid' || style === 'Rings') {
                for (const wave of waves) {
                    const color = mixRgb(peak, accent, wave.mix)
                    const ringAlpha = 0.18 + wave.strength * 0.68
                    const ringWidth = s.ds(2 + ((controls.ringWidth as number) / 60) * 22)
                    const ringRadius = s.ds(14) + wave.radius
                    ctx.save()
                    // Halo pass: a wider, dimmer stroke stands in for the old
                    // shadowBlur glow, which forced a full blur per wave on
                    // Servo's software canvas.
                    ctx.globalAlpha = ringAlpha * (0.16 + glow * 0.24)
                    ctx.lineWidth = ringWidth + s.ds(8 + glow * 34)
                    ctx.strokeStyle = rgbToCss(color, 1)
                    ctx.beginPath()
                    ctx.arc(centerX, centerY, ringRadius, 0, Math.PI * 2)
                    ctx.stroke()

                    ctx.globalAlpha = ringAlpha
                    ctx.lineWidth = ringWidth
                    ctx.strokeStyle = rgbToCss(withLift(color, glow * 0.22))
                    ctx.beginPath()
                    ctx.arc(centerX, centerY, ringRadius, 0, Math.PI * 2)
                    ctx.stroke()
                    ctx.restore()
                }
            }

            ctx.restore()

            const vignette = ctx.createRadialGradient(centerX, centerY, s.ds(40), centerX, centerY, s.ds(210))
            vignette.addColorStop(0, 'rgba(0, 0, 0, 0)')
            vignette.addColorStop(1, `rgba(0, 0, 0, ${0.18 + (1 - brightness) * 0.32})`)
            ctx.fillStyle = vignette
            ctx.fillRect(0, 0, width, height)
        }
    },
    {
        author: 'Hypercolor',
        audio: true,
        builtinId: 'audio_pulse',
        category: 'audio',
        description:
            'Beat-reactive blooms and expanding rings with layered color, bass presence, and presets tuned for music.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    accentColor: '#80ffea',
                    baseColor: '#070b18',
                    brightness: 82,
                    floor: 16,
                    glow: 58,
                    linger: 62,
                    peakColor: '#ff4f9a',
                    ringWidth: 20,
                    sensitivity: 110,
                    speed: 52,
                    style: 'Hybrid',
                },
                description: 'A plush heartbeat with crisp rings on the downbeat and a soft neon body between kicks.',
                name: 'Silk Heartbeat',
            },
            {
                controls: {
                    accentColor: '#ffcc33',
                    baseColor: '#10040c',
                    brightness: 92,
                    floor: 24,
                    glow: 72,
                    linger: 48,
                    peakColor: '#ff5959',
                    ringWidth: 14,
                    sensitivity: 135,
                    speed: 68,
                    style: 'Pulse Rays',
                },
                description:
                    'Sharper, hotter transient response for alarms, industrial techno, and dramatic chorus hits.',
                name: 'Neon Siren',
            },
            {
                controls: {
                    accentColor: '#63a9ff',
                    baseColor: '#030812',
                    brightness: 74,
                    floor: 10,
                    glow: 44,
                    linger: 80,
                    peakColor: '#6e6cff',
                    ringWidth: 28,
                    sensitivity: 92,
                    speed: 34,
                    style: 'Bloom',
                },
                description: 'Low-light sub-bass ambience that hangs in the room between kicks.',
                name: 'Midnight Sub',
            },
            {
                controls: {
                    accentColor: '#7dffae',
                    baseColor: '#061109',
                    brightness: 86,
                    floor: 18,
                    glow: 64,
                    linger: 46,
                    peakColor: '#36ff9a',
                    ringWidth: 18,
                    sensitivity: 128,
                    speed: 74,
                    style: 'Rings',
                },
                description: 'Clean kinetic rings for tempo mapping, DJ cues, and watching the beat tracking breathe.',
                name: 'Pulse Scope',
            },
        ],
    },
)
