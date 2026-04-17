import type { AudioData } from '@hypercolor/sdk'
import { audio, canvas, combo, num } from '@hypercolor/sdk'

type SceneName = (typeof SCENES)[number]
type PaletteName = (typeof PALETTE_NAMES)[number]

interface Rgb {
    r: number
    g: number
    b: number
}

interface Wavefront {
    kind: 'arc' | 'diamond'
    x: number
    y: number
    radius: number
    speed: number
    width: number
    age: number
    life: number
    colorPhase: number
    segmentCount: number
    rotation: number
    sweep: number
}

interface SpokeBurst {
    kind: 'spokes'
    x: number
    y: number
    radius: number
    speed: number
    width: number
    age: number
    life: number
    colorPhase: number
    spokeCount: number
    rotation: number
}

interface BridgeBand {
    kind: 'bridge'
    y: number
    speed: number
    thickness: number
    age: number
    life: number
    colorPhase: number
    skew: number
    direction: number
}

interface CascadeBand {
    kind: 'cascade'
    y: number
    speed: number
    thickness: number
    age: number
    life: number
    colorPhase: number
    drift: number
}

type Accent = SpokeBurst | BridgeBand | CascadeBand

const SCENES = ['Cascade', 'Core Burst', 'Twin Burst'] as const
const PALETTE_NAMES = ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit'] as const
const TAU = Math.PI * 2

const LED_PALETTES: Record<PaletteName, readonly Rgb[]> = {
    Aurora: [
        { b: 255, g: 229, r: 0 },
        { b: 80, g: 175, r: 76 },
        { b: 255, g: 77, r: 124 },
        { b: 165, g: 191, r: 0 },
    ],
    Cyberpunk: [
        { b: 255, g: 0, r: 255 },
        { b: 255, g: 255, r: 0 },
        { b: 102, g: 0, r: 255 },
        { b: 255, g: 0, r: 102 },
    ],
    Fire: [
        { b: 0, g: 48, r: 255 },
        { b: 0, g: 106, r: 255 },
        { b: 0, g: 148, r: 255 },
        { b: 0, g: 20, r: 191 },
    ],
    Ice: [
        { b: 161, g: 71, r: 13 },
        { b: 255, g: 229, r: 0 },
        { b: 255, g: 140, r: 0 },
        { b: 255, g: 96, r: 48 },
    ],
    SilkCircuit: [
        { b: 255, g: 53, r: 225 },
        { b: 234, g: 255, r: 128 },
        { b: 193, g: 106, r: 255 },
        { b: 123, g: 250, r: 80 },
    ],
}

function clamp(v: number, lo: number, hi: number): number {
    return Math.max(lo, Math.min(hi, v))
}

function fract(v: number): number {
    return v - Math.floor(v)
}

function randomBetween(lo: number, hi: number): number {
    return lo + Math.random() * (hi - lo)
}

function rgbString(color: Rgb, brightness = 1): string {
    const level = clamp(brightness, 0, 1) ** 0.92
    const r = Math.round(color.r * level)
    const g = Math.round(color.g * level)
    const b = Math.round(color.b * level)
    return `rgb(${r},${g},${b})`
}

function resolvePaletteName(): PaletteName {
    const raw = String((globalThis as Record<string, unknown>).palette ?? 'SilkCircuit')
    return PALETTE_NAMES.includes(raw as PaletteName) ? (raw as PaletteName) : 'SilkCircuit'
}

function samplePaletteColor(paletteName: PaletteName, phase: number): Rgb {
    const colors = LED_PALETTES[paletteName]
    const index = Math.floor(fract(phase) * colors.length) % colors.length
    return colors[index] ?? LED_PALETTES.SilkCircuit[0]
}

function trimEntities<T>(items: T[], maxItems: number): T[] {
    return items.length <= maxItems ? items : items.slice(items.length - maxItems)
}

function emitterPositions(scene: SceneName, w: number, h: number): [number, number][] {
    const cx = w * 0.5
    const cy = h * 0.5

    if (scene === 'Twin Burst') {
        return [
            [cx - w * 0.22, cy],
            [cx + w * 0.22, cy],
        ]
    }

    if (scene === 'Cascade') {
        return [
            [cx, h * 0.16],
            [cx - w * 0.27, h * 0.58],
            [cx + w * 0.27, h * 0.58],
        ]
    }

    return [[cx, cy]]
}

function resolveAudio(
    a: AudioData,
    fallbackPhase: number,
): {
    shouldSpawn: boolean
    pulse: number
    motion: number
} {
    const audioPresent = a.level > 0.03 || a.bass > 0.03 || a.mid > 0.03

    if (audioPresent) {
        const pulse = clamp(Math.max(a.bass, a.beatPulse * 0.85, a.onsetPulse * 0.75), 0, 1)
        const motion = clamp(Math.max(a.mid * 0.8, a.treble, a.level), 0, 1)
        return {
            motion,
            pulse,
            shouldSpawn: a.beatPulse > 0.38 || a.onsetPulse > 0.48 || a.bass > 0.62,
        }
    }

    const syntheticBeat = Math.max(0, Math.sin(fallbackPhase * 1.6)) ** 8
    const motion = 0.3 + (0.5 + 0.5 * Math.sin(fallbackPhase * 0.9)) * 0.35
    return {
        motion,
        pulse: syntheticBeat * 0.75,
        shouldSpawn: syntheticBeat > 0.7,
    }
}

function spawnWavefront(
    scene: SceneName,
    x: number,
    y: number,
    w: number,
    speed: number,
    intensity: number,
    decay: number,
    density: number,
    colorPhase: number,
): Wavefront {
    const persistence = 0.55 + (1 - decay) * 0.95
    const speedMul = speed / 10
    const baseWidth = 8 + intensity * 8 + randomBetween(0, 4)

    if (scene === 'Cascade') {
        return {
            age: 0,
            colorPhase,
            kind: 'diamond',
            life: persistence + randomBetween(0.08, 0.28),
            radius: 10,
            rotation: Math.PI * 0.25,
            segmentCount: 4,
            speed: 58 + speedMul * 42 + randomBetween(0, 18),
            sweep: TAU,
            width: baseWidth * 0.9,
            x,
            y,
        }
    }

    const sweep = scene === 'Twin Burst' ? Math.PI * 0.92 : TAU * 0.94
    const segmentCount = scene === 'Twin Burst' ? 4 + Math.round(density * 3) : 6 + Math.round(density * 4)
    const heading = scene === 'Twin Burst' ? (x < w * 0.5 ? Math.PI : 0) : randomBetween(0, TAU)

    return {
        age: 0,
        colorPhase,
        kind: 'arc',
        life: persistence + randomBetween(0.12, 0.34),
        radius: 10,
        rotation: heading,
        segmentCount,
        speed: 72 + speedMul * 50 + randomBetween(0, 25),
        sweep,
        width: baseWidth,
        x,
        y,
    }
}

function spawnSpokeBurst(
    x: number,
    y: number,
    speed: number,
    intensity: number,
    decay: number,
    density: number,
    colorPhase: number,
): SpokeBurst {
    const persistence = 0.35 + (1 - decay) * 0.45
    return {
        age: 0,
        colorPhase,
        kind: 'spokes',
        life: persistence + randomBetween(0.04, 0.14),
        radius: 18,
        rotation: randomBetween(0, TAU),
        speed: 90 + speed * 7 + density * 22,
        spokeCount: 6 + Math.round(density * 4),
        width: 5 + intensity * 3,
        x,
        y,
    }
}

function spawnBridgeBand(
    y: number,
    speed: number,
    intensity: number,
    decay: number,
    direction: number,
    colorPhase: number,
): BridgeBand {
    const persistence = 0.4 + (1 - decay) * 0.4
    return {
        age: 0,
        colorPhase,
        direction,
        kind: 'bridge',
        life: persistence + randomBetween(0.08, 0.16),
        skew: randomBetween(10, 22),
        speed: 0.65 + speed * 0.08,
        thickness: 10 + intensity * 7,
        y,
    }
}

function spawnCascadeBand(y: number, speed: number, intensity: number, decay: number, colorPhase: number): CascadeBand {
    const persistence = 0.45 + (1 - decay) * 0.5
    return {
        age: 0,
        colorPhase,
        drift: randomBetween(12, 36),
        kind: 'cascade',
        life: persistence + randomBetween(0.08, 0.2),
        speed: 72 + speed * 8,
        thickness: 12 + intensity * 8,
        y,
    }
}

function drawSegmentedArc(ctx: CanvasRenderingContext2D, wave: Wavefront, color: string): void {
    const segments = Math.max(3, wave.segmentCount)
    const segmentArc = wave.sweep / segments
    const visibleArc = segmentArc * 0.62
    const arcStart = wave.rotation - wave.sweep * 0.5

    ctx.strokeStyle = color
    ctx.lineWidth = wave.width
    ctx.lineCap = 'square'

    for (let index = 0; index < segments; index += 1) {
        const start = arcStart + index * segmentArc
        ctx.beginPath()
        ctx.arc(wave.x, wave.y, wave.radius, start, start + visibleArc)
        ctx.stroke()
    }
}

function drawDiamondBand(ctx: CanvasRenderingContext2D, wave: Wavefront, color: string): void {
    const radius = wave.radius

    ctx.strokeStyle = color
    ctx.lineWidth = wave.width
    ctx.lineJoin = 'miter'
    ctx.beginPath()
    ctx.moveTo(wave.x, wave.y - radius)
    ctx.lineTo(wave.x + radius * 1.15, wave.y)
    ctx.lineTo(wave.x, wave.y + radius)
    ctx.lineTo(wave.x - radius * 1.15, wave.y)
    ctx.closePath()
    ctx.stroke()
}

function drawWavefront(ctx: CanvasRenderingContext2D, wave: Wavefront, paletteName: PaletteName, motion: number): void {
    const lifeFrac = clamp(wave.age / wave.life, 0, 1)
    if (lifeFrac >= 1) return

    const brightness = 0.3 + (1 - lifeFrac) * (0.55 + motion * 0.25)
    const color = rgbString(samplePaletteColor(paletteName, wave.colorPhase + lifeFrac * 0.22), brightness)

    if (wave.kind === 'diamond') {
        drawDiamondBand(ctx, wave, color)
        return
    }

    drawSegmentedArc(ctx, wave, color)
}

function drawSpokeBurst(
    ctx: CanvasRenderingContext2D,
    burst: SpokeBurst,
    paletteName: PaletteName,
    pulse: number,
): void {
    const lifeFrac = clamp(burst.age / burst.life, 0, 1)
    if (lifeFrac >= 1) return

    const brightness = 0.35 + (1 - lifeFrac) * (0.55 + pulse * 0.2)
    ctx.strokeStyle = rgbString(samplePaletteColor(paletteName, burst.colorPhase), brightness)
    ctx.lineWidth = burst.width
    ctx.lineCap = 'square'

    const innerRadius = 10 + lifeFrac * 12
    const outerRadius = burst.radius

    for (let index = 0; index < burst.spokeCount; index += 1) {
        const angle = burst.rotation + (index / burst.spokeCount) * TAU
        const x1 = burst.x + Math.cos(angle) * innerRadius
        const y1 = burst.y + Math.sin(angle) * innerRadius
        const x2 = burst.x + Math.cos(angle) * outerRadius
        const y2 = burst.y + Math.sin(angle) * outerRadius

        ctx.beginPath()
        ctx.moveTo(x1, y1)
        ctx.lineTo(x2, y2)
        ctx.stroke()
    }
}

function drawBridgeBand(ctx: CanvasRenderingContext2D, band: BridgeBand, paletteName: PaletteName, w: number): void {
    const lifeFrac = clamp(band.age / band.life, 0, 1)
    if (lifeFrac >= 1) return

    const brightness = 0.3 + (1 - lifeFrac) * 0.65
    const color = rgbString(samplePaletteColor(paletteName, band.colorPhase + lifeFrac * 0.12), brightness)
    const travel = clamp(lifeFrac * band.speed, 0, 1)
    const progress = band.direction > 0 ? travel : 1 - travel
    const headX = progress * (w + band.thickness * 2) - band.thickness
    const tailX = headX - band.direction * (w * 0.28)

    ctx.fillStyle = color
    ctx.beginPath()
    ctx.moveTo(tailX, band.y - band.thickness)
    ctx.lineTo(headX, band.y - band.thickness - band.skew)
    ctx.lineTo(headX + band.direction * band.thickness * 1.5, band.y)
    ctx.lineTo(headX, band.y + band.thickness + band.skew)
    ctx.lineTo(tailX, band.y + band.thickness)
    ctx.closePath()
    ctx.fill()
}

function drawCascadeBand(ctx: CanvasRenderingContext2D, band: CascadeBand, paletteName: PaletteName, w: number): void {
    const lifeFrac = clamp(band.age / band.life, 0, 1)
    if (lifeFrac >= 1) return

    const brightness = 0.32 + (1 - lifeFrac) * 0.62
    const color = rgbString(samplePaletteColor(paletteName, band.colorPhase + lifeFrac * 0.16), brightness)
    const y = band.y + band.speed * band.age
    const inset = 20 + band.drift * lifeFrac

    ctx.fillStyle = color
    ctx.beginPath()
    ctx.moveTo(inset, y - band.thickness)
    ctx.lineTo(w * 0.5, y - band.thickness * 1.6)
    ctx.lineTo(w - inset, y - band.thickness)
    ctx.lineTo(w - inset - 18, y + band.thickness)
    ctx.lineTo(inset + 18, y + band.thickness)
    ctx.closePath()
    ctx.fill()
}

function drawEmitterCore(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    paletteName: PaletteName,
    phase: number,
    pulse: number,
): void {
    const outer = 8 + pulse * 9
    const inner = 4 + pulse * 4

    ctx.fillStyle = rgbString(samplePaletteColor(paletteName, phase), 0.45 + pulse * 0.35)
    ctx.beginPath()
    ctx.moveTo(x, y - outer)
    ctx.lineTo(x + outer, y)
    ctx.lineTo(x, y + outer)
    ctx.lineTo(x - outer, y)
    ctx.closePath()
    ctx.fill()

    ctx.fillStyle = rgbString(samplePaletteColor(paletteName, phase + 0.25), 0.6 + pulse * 0.25)
    ctx.fillRect(x - inner, y - inner, inner * 2, inner * 2)
}

export default canvas.stateful(
    'Shockwave',
    {
        palette: combo('Palette', ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit'], {
            default: 'SilkCircuit',
            group: 'Scene',
        }),
        scene: combo('Scene', [...SCENES], { default: 'Cascade', group: 'Scene' }),
        speed: num('Speed', [1, 10], 6, { group: 'Motion' }),
        intensity: num('Intensity', [0, 100], 78, { group: 'Motion' }),
        decay: num('Decay', [0, 100], 52, { group: 'Motion' }),
        ringCount: num('Ring Count', [2, 12], 6, { group: 'Geometry' }),
    },
    () => {
        let waves: Wavefront[] = []
        let accents: Accent[] = []
        let lastTime = -1
        let beatCooldown = 0
        let fallbackPhase = 0
        let colorPhase = 0

        return (ctx, time, controls) => {
            const speed = controls.speed as number
            const intensity = clamp((controls.intensity as number) / 100, 0, 1)
            const decay = clamp((controls.decay as number) / 100, 0, 1)
            const maxRings = Math.max(2, Math.round(controls.ringCount as number))
            const sceneRaw = String(controls.scene ?? SCENES[0])
            const scene = SCENES.includes(sceneRaw as SceneName) ? (sceneRaw as SceneName) : SCENES[0]
            const paletteName = resolvePaletteName()

            const w = ctx.canvas.width
            const h = ctx.canvas.height
            const dt = lastTime < 0 ? 1 / 60 : Math.min(0.05, time - lastTime)
            lastTime = time

            fallbackPhase += dt * (0.8 + speed * 0.3)

            const analysis = resolveAudio(audio(), fallbackPhase)
            const density = clamp(maxRings / 12, 0.2, 1)
            const emitters = emitterPositions(scene, w, h)

            beatCooldown = Math.max(0, beatCooldown - dt)
            if (analysis.shouldSpawn && beatCooldown <= 0) {
                for (const [x, y] of emitters) {
                    waves.push(spawnWavefront(scene, x, y, w, speed, intensity, decay, density, colorPhase))
                    colorPhase += 0.18
                }

                if (scene === 'Core Burst') {
                    const [x, y] = emitters[0] ?? [w * 0.5, h * 0.5]
                    accents.push(spawnSpokeBurst(x, y, speed, intensity, decay, density, colorPhase))
                    colorPhase += 0.21
                } else if (scene === 'Twin Burst') {
                    accents.push(spawnBridgeBand(h * 0.5, speed, intensity, decay, 1, colorPhase))
                    colorPhase += 0.17
                    accents.push(spawnBridgeBand(h * 0.5, speed, intensity, decay, -1, colorPhase))
                    colorPhase += 0.17
                } else {
                    accents.push(spawnCascadeBand(h * 0.18, speed, intensity, decay, colorPhase))
                    colorPhase += 0.16
                    accents.push(spawnCascadeBand(h * 0.36, speed, intensity, decay, colorPhase))
                    colorPhase += 0.16
                }

                waves = trimEntities(waves, maxRings * emitters.length * 2)
                accents = trimEntities(accents, maxRings * 3)
                beatCooldown = 0.08 + (1 - speed / 10) * 0.14
            }

            ctx.fillStyle = 'rgb(0,0,0)'
            ctx.fillRect(0, 0, w, h)

            for (const wave of waves) {
                wave.age += dt
                wave.radius += wave.speed * dt * (1 + analysis.pulse * 0.25)
                wave.rotation += dt * (0.2 + analysis.motion * 0.35)
                drawWavefront(ctx, wave, paletteName, analysis.motion)
            }

            for (const accent of accents) {
                accent.age += dt

                if (accent.kind === 'spokes') {
                    accent.radius += accent.speed * dt * (1 + analysis.motion * 0.15)
                    accent.rotation += dt * (0.4 + analysis.motion * 0.8)
                    drawSpokeBurst(ctx, accent, paletteName, analysis.pulse)
                } else if (accent.kind === 'bridge') {
                    drawBridgeBand(ctx, accent, paletteName, w)
                } else {
                    drawCascadeBand(ctx, accent, paletteName, w)
                }
            }

            emitters.forEach(([x, y], index) => {
                drawEmitterCore(ctx, x, y, paletteName, colorPhase + index * 0.22, analysis.pulse)
            })

            waves = waves.filter((wave) => wave.age < wave.life)
            accents = accents.filter((accent) => accent.age < accent.life)
        }
    },
    {
        description:
            'Bass drops detonate concentric shockwaves. Segmented bursts and cascade chevrons ripple outward on every impact.',
        presets: [
            {
                controls: {
                    decay: 35,
                    intensity: 95,
                    palette: 'Fire',
                    ringCount: 10,
                    scene: 'Core Burst',
                    speed: 7,
                },
                description:
                    'Stand at ground zero of a tectonic rupture. Massive concentric shockwaves tear outward from a single point as fire-colored rings decay into the abyss.',
                name: 'Seismic Epicenter',
            },
            {
                controls: {
                    decay: 75,
                    intensity: 60,
                    palette: 'Ice',
                    ringCount: 4,
                    scene: 'Cascade',
                    speed: 3,
                },
                description:
                    'An ice shelf fractures in slow motion. Wide, cold shockwaves cascade downward like frozen thunder while sparse rings hold their shape before dissolving.',
                name: 'Glacier Calving',
            },
            {
                controls: {
                    decay: 42,
                    intensity: 88,
                    palette: 'Cyberpunk',
                    ringCount: 8,
                    scene: 'Twin Burst',
                    speed: 8,
                },
                description:
                    'Two containment fields fail simultaneously. Mirrored shockwaves collide in a corridor of neon plasma as bridge bands sweep between the rupture points.',
                name: 'Twin Reactor Breach',
            },
            {
                controls: {
                    decay: 68,
                    intensity: 55,
                    palette: 'Aurora',
                    ringCount: 6,
                    scene: 'Cascade',
                    speed: 4,
                },
                description:
                    'The northern lights break apart like stained glass. Gentle waves ripple through green and violet as rings dissolve into the polar dark.',
                name: 'Aurora Shatter',
            },
            {
                controls: {
                    decay: 20,
                    intensity: 100,
                    palette: 'SilkCircuit',
                    ringCount: 12,
                    scene: 'Core Burst',
                    speed: 10,
                },
                description:
                    'A digital bomb detonates inside a circuit board. Maximum ring density, rapid-fire bursts of electric purple and cyan, spoke patterns spinning through the debris.',
                name: 'SilkCircuit Detonation',
            },
            {
                controls: {
                    decay: 90,
                    intensity: 30,
                    palette: 'Ice',
                    ringCount: 2,
                    scene: 'Twin Burst',
                    speed: 1,
                },
                description:
                    'Two frozen sigils pulse once and hold. Glacial rings linger in the void like the last heartbeat of a dying star.',
                name: 'Event Horizon',
            },
            {
                controls: {
                    decay: 15,
                    intensity: 72,
                    palette: 'Fire',
                    ringCount: 12,
                    scene: 'Cascade',
                    speed: 9,
                },
                description:
                    'Molten chevrons pour down the screen in relentless waves. Every beat hammers another cascade of burning geometry into the dark.',
                name: 'Forge Hammer',
            },
        ],
    },
)
