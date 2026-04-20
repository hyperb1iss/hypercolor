import type { AudioData } from '@hypercolor/sdk'
import { audio, canvas, combo, num } from '@hypercolor/sdk'

type SceneName = (typeof SCENES)[number]
type PaletteName = (typeof PALETTE_NAMES)[number]

interface Rgb {
    r: number
    g: number
    b: number
}

// Ripple that rides outward through the banded field.
interface Burst {
    emitterIndex: number
    age: number
    life: number
    speed: number
    strength: number
    hueShift: number
}

// Bright annular pressure wave. Stroked circle — no alpha needed.
interface FlashRing {
    x: number
    y: number
    age: number
    life: number
    maxR: number
    hueOffset: number
    intensity: number
    emitterIndex: number
}

const SCENES = ['Cascade', 'Core Burst', 'Twin Burst'] as const
const PALETTE_NAMES = ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit'] as const
const TAU = Math.PI * 2

// 5-stop gradients ordered for smooth continuous cycling.
// Index 0 is the darkest "ground" tone — used as the canvas base and mandala cuts.
const LED_PALETTES: Record<PaletteName, readonly Rgb[]> = {
    Aurora: [
        { r: 4, g: 14, b: 34 },
        { r: 26, g: 110, b: 184 },
        { r: 46, g: 200, b: 148 },
        { r: 176, g: 255, b: 124 },
        { r: 96, g: 60, b: 220 },
    ],
    Cyberpunk: [
        { r: 12, g: 0, b: 28 },
        { r: 136, g: 0, b: 255 },
        { r: 255, g: 40, b: 144 },
        { r: 255, g: 236, b: 48 },
        { r: 104, g: 0, b: 188 },
    ],
    Fire: [
        { r: 20, g: 0, b: 0 },
        { r: 172, g: 16, b: 0 },
        { r: 255, g: 104, b: 0 },
        { r: 255, g: 204, b: 44 },
        { r: 148, g: 0, b: 28 },
    ],
    Ice: [
        { r: 2, g: 10, b: 32 },
        { r: 0, g: 88, b: 216 },
        { r: 44, g: 192, b: 255 },
        { r: 196, g: 244, b: 255 },
        { r: 54, g: 36, b: 176 },
    ],
    SilkCircuit: [
        { r: 20, g: 0, b: 44 },
        { r: 200, g: 48, b: 224 },
        { r: 128, g: 255, b: 234 },
        { r: 255, g: 106, b: 193 },
        { r: 224, g: 92, b: 255 },
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

function resolvePaletteName(raw: unknown): PaletteName {
    const name = String(raw ?? 'SilkCircuit')
    return PALETTE_NAMES.includes(name as PaletteName) ? (name as PaletteName) : 'SilkCircuit'
}

function resolveSceneName(raw: unknown): SceneName {
    const name = String(raw ?? 'Core Burst')
    return SCENES.includes(name as SceneName) ? (name as SceneName) : 'Core Burst'
}

// Sharp threshold keeps each ring parked on a palette key ~90% of the time
// and only briefly transits through muddy inter-key mixes. LED-friendly.
function softKey(f: number): number {
    const e0 = 0.46
    const e1 = 0.54
    const t = clamp((f - e0) / (e1 - e0), 0, 1)
    return t * t * (3 - 2 * t)
}

function samplePalette(pal: readonly Rgb[], t: number, brightness: number): string {
    const n = pal.length
    const scaled = fract(t) * n
    const i0 = Math.floor(scaled) % n
    const i1 = (i0 + 1) % n
    const raw = scaled - Math.floor(scaled)
    const f = softKey(raw)
    const a = pal[i0]
    const b = pal[i1]
    const br = clamp(brightness, 0, 1)
    const r = Math.round((a.r * (1 - f) + b.r * f) * br)
    const g = Math.round((a.g * (1 - f) + b.g * f) * br)
    const c = Math.round((a.b * (1 - f) + b.b * f) * br)
    return `rgb(${r},${g},${c})`
}

function rgbCss(color: Rgb): string {
    return `rgb(${color.r},${color.g},${color.b})`
}

interface AudioState {
    level: number
    bass: number
    treble: number
    pulse: number
    onset: number
    spawnNow: boolean
    present: boolean
}

function analyzeAudio(a: AudioData, synthPhase: number): AudioState {
    const present = a.level > 0.025 || a.bass > 0.03 || a.mid > 0.03

    if (present) {
        const pulse = clamp(Math.max(a.beatPulse, a.onsetPulse * 0.8, a.bass * 0.6), 0, 1)
        return {
            bass: clamp(Math.max(a.bassEnv, a.bass), 0, 1),
            level: clamp(Math.max(a.levelShort, a.level), 0, 1),
            onset: clamp(a.onsetPulse, 0, 1),
            present: true,
            pulse,
            // Lower thresholds so mid-range music keeps the field dancing.
            spawnNow: a.beatPulse > 0.22 || a.onsetPulse > 0.32 || a.bass > 0.48,
            treble: clamp(Math.max(a.trebleEnv, a.treble), 0, 1),
        }
    }

    const synthBeat = Math.max(0, Math.sin(synthPhase * 1.65)) ** 7 * 0.78
    return {
        bass: 0.16 + Math.sin(synthPhase * 0.72) * 0.08,
        level: 0.2 + Math.sin(synthPhase * 0.36) * 0.06,
        onset: synthBeat,
        present: false,
        pulse: synthBeat,
        spawnNow: synthBeat > 0.35,
        treble: 0.14 + Math.sin(synthPhase * 0.94) * 0.06,
    }
}

// A burst rides outward through the rings. Peak lives at ringT = age/life.
const RIPPLE_WIDTH = 0.13

function burstPosition(burst: Burst): number {
    return clamp(burst.age / burst.life, 0, 1)
}

function rippleMagnitude(burst: Burst, ringT: number): number {
    const d = ringT - burstPosition(burst)
    return Math.exp(-(d * d) / (RIPPLE_WIDTH * RIPPLE_WIDTH))
}

interface RingModulation {
    radiusBulge: number
    brightnessBoost: number
    hueNudge: number
}

function collectBurstModulation(
    bursts: readonly Burst[],
    ringT: number,
    emitterIndex: number,
): RingModulation {
    let radiusBulge = 0
    let brightnessBoost = 0
    let hueNudge = 0

    for (const burst of bursts) {
        if (burst.emitterIndex >= 0 && burst.emitterIndex !== emitterIndex) continue
        const envelope = 1 - burstPosition(burst)
        const mag = rippleMagnitude(burst, ringT) * envelope * burst.strength
        radiusBulge += mag * 0.18
        brightnessBoost += mag * 1.0
        hueNudge += mag * burst.hueShift * 0.22
    }

    return { brightnessBoost, hueNudge, radiusBulge }
}

interface RingDrawArgs {
    ctx: CanvasRenderingContext2D
    cx: number
    cy: number
    maxR: number
    ringCount: number
    ringIndex: number
    pal: readonly Rgb[]
    phase: number
    flowTime: number
    time: number
    audio: AudioState
    bursts: readonly Burst[]
    emitterIndex: number
    intensityScale: number
    hueBase: number
    fieldScale: number
    rotationBase: number
}

function drawBullseyeRing(args: RingDrawArgs): void {
    const {
        ctx, cx, cy, maxR, ringCount, ringIndex, pal, phase, flowTime, time, audio,
        bursts, emitterIndex, intensityScale, hueBase, fieldScale, rotationBase,
    } = args
    const t = ringIndex / ringCount
    const baseR = t * maxR * fieldScale
    const mod = collectBurstModulation(bursts, t, emitterIndex)

    // Harmonic breathing — multiple frequencies superposed per ring give organic,
    // non-periodic radius wobble instead of a single visible sine.
    const breathe = (
        Math.sin(time * 0.62 + ringIndex * 0.73) * 0.7 +
        Math.sin(time * 1.37 + ringIndex * 1.21) * 0.35 +
        Math.sin(time * 0.27 + ringIndex * 0.41) * 0.45
    ) * maxR * 0.008

    const r = baseR + breathe + maxR * mod.radiusBulge
    if (r <= 0) return

    // Per-ring orbit: each ring drifts in a small circle around the main center.
    // Outer rings orbit further, inner rings stay anchored — creates lens-distortion wobble.
    const orbitR = maxR * 0.014 * (t + 0.25)
    const orbitPhase = time * (0.18 + ringIndex * 0.023) + ringIndex * 1.37
    const ringCx = cx + Math.cos(orbitPhase) * orbitR
    const ringCy = cy + Math.sin(orbitPhase) * orbitR * 0.85

    // Continuous outward flow: colors cascade from inner to outer over time.
    // Per-ring phase offset breaks lockstep so rings don't all change color at once.
    const perRingPhaseOffset = ringIndex * 0.043
    const hue =
        hueBase + phase + t * 0.62 + flowTime + perRingPhaseOffset + mod.hueNudge +
        Math.sin(time * 0.4 + ringIndex * 0.45) * 0.028

    const ambient = 0.58 + (1 - t) * 0.38
    const brightness = clamp((ambient + audio.level * 0.25 + mod.brightnessBoost) * intensityScale, 0, 1)

    // Elliptical rings with independently rotating axes per ring.
    // Counter-rotation (alternating direction) produces shearing/shimmer motion.
    const ellipticity = 0.05 + audio.bass * 0.22 + audio.pulse * 0.1
    const ry = r * (1 - ellipticity)
    const direction = ringIndex % 2 === 0 ? 1 : -0.7
    const ringRotRate = 0.55 + ((ringIndex % 3) * 0.18)
    const ringRot =
        rotationBase * direction * ringRotRate +
        ringIndex * 0.33 +
        Math.sin(time * 0.19 + ringIndex * 0.9) * 0.09

    ctx.fillStyle = samplePalette(pal, hue, brightness)
    ctx.beginPath()
    ctx.ellipse(ringCx, ringCy, r, ry, ringRot, 0, TAU)
    ctx.fill()
}

function drawBullseye(args: Omit<RingDrawArgs, 'ringIndex'>): void {
    // Outer-to-inner: each smaller disk overwrites the last. Opaque bands, no alpha.
    for (let i = args.ringCount; i >= 0; i -= 1) {
        drawBullseyeRing({ ...args, ringIndex: i })
    }
}

function drawMandalaCuts(
    ctx: CanvasRenderingContext2D,
    cx: number,
    cy: number,
    maxR: number,
    spokes: number,
    rotation: number,
    cutColor: string,
    wedgeFrac: number,
    time: number,
    bass: number,
): void {
    const spokeAng = TAU / spokes
    ctx.fillStyle = cutColor
    for (let i = 0; i < spokes; i += 1) {
        // Per-wedge width oscillation — each wedge breathes at its own phase so
        // the mandala looks like it's exhaling unevenly rather than spinning rigid.
        const widthMul = 0.62 + Math.sin(time * 0.85 + i * 1.37) * 0.35 + bass * 0.25
        const half = spokeAng * wedgeFrac * widthMul
        // Alternating short/long wedge lengths break the solid pinwheel silhouette.
        const lenMul = i % 3 === 2 ? 0.5 + Math.sin(time * 0.6 + i) * 0.08 : 1.0
        const a = rotation + i * spokeAng
        const r = maxR * 1.3 * lenMul
        ctx.beginPath()
        ctx.moveTo(cx, cy)
        ctx.arc(cx, cy, r, a - half, a + half)
        ctx.closePath()
        ctx.fill()
    }
}

function drawBrightRays(
    ctx: CanvasRenderingContext2D,
    cx: number,
    cy: number,
    maxR: number,
    spokes: number,
    primaryRotation: number,
    accentRotation: number,
    pal: readonly Rgb[],
    phase: number,
    time: number,
    pulse: number,
    rawPulse: number,
    bass: number,
    treble: number,
    intensityScale: number,
): void {
    ctx.lineCap = 'round'

    // Primary rays: count equals spoke count, extend in lockstep with the beat.
    // These are the disciplined "shockwave lances" that read as the pulse.
    const primaryAng = TAU / spokes
    const primaryInner = maxR * 0.08
    const primaryOuter = maxR * 0.92
    const primaryWidth = Math.max(3, maxR * 0.007 * (1 + pulse * 1.4))
    const primaryBrightness = clamp(0.88 * intensityScale + rawPulse * 0.12, 0, 1)
    for (let i = 0; i < spokes; i += 1) {
        const a = primaryRotation + i * primaryAng
        // Coordinated length — all primary rays stretch together on beats.
        // Small per-ray variation keeps the pattern from feeling mechanical.
        const lenMod = 0.52 + pulse * 0.38 + rawPulse * 0.18 + bass * 0.1 +
            Math.sin(time * 0.28 + i * 0.47) * 0.07
        const outerR = primaryOuter * clamp(lenMod, 0.3, 1.2)
        const hue = phase + 0.34 + (i / spokes) * 0.18
        ctx.strokeStyle = samplePalette(pal, hue, primaryBrightness)
        ctx.lineWidth = primaryWidth
        ctx.beginPath()
        ctx.moveTo(cx + Math.cos(a) * primaryInner, cy + Math.sin(a) * primaryInner)
        ctx.lineTo(cx + Math.cos(a) * outerR, cy + Math.sin(a) * outerR)
        ctx.stroke()
    }

    // Accent rays: half the count, offset between primaries, thinner and more
    // independent. These keep the ambient shimmer alive between beats.
    const accentCount = Math.max(2, Math.floor(spokes / 2))
    const accentAng = TAU / accentCount
    const accentInner = maxR * 0.1
    const accentOuter = maxR * 0.62
    const accentWidth = Math.max(1.2, maxR * 0.0034)
    const accentBrightness = clamp(0.6 * intensityScale + treble * 0.22, 0, 1)
    for (let i = 0; i < accentCount; i += 1) {
        const a = accentRotation + i * accentAng + primaryAng * 0.5
        const lenMod = 0.35 + Math.sin(time * 1.15 + i * 1.71) * 0.32 + treble * 0.3 + bass * 0.08
        const outerR = accentOuter * clamp(lenMod, 0.2, 1.1)
        const hue = phase + 0.55 + (i / accentCount) * 0.28
        ctx.strokeStyle = samplePalette(pal, hue, accentBrightness)
        ctx.lineWidth = accentWidth
        ctx.beginPath()
        ctx.moveTo(cx + Math.cos(a) * accentInner, cy + Math.sin(a) * accentInner)
        ctx.lineTo(cx + Math.cos(a) * outerR, cy + Math.sin(a) * outerR)
        ctx.stroke()
    }
}

function drawFlashRing(
    ctx: CanvasRenderingContext2D,
    ring: FlashRing,
    pal: readonly Rgb[],
    phase: number,
    intensityScale: number,
): void {
    const f = clamp(ring.age / ring.life, 0, 1)
    if (f >= 1) return

    const radius = f * ring.maxR
    // Width shrinks as the ring expands — the front is thinnest and brightest.
    const width = Math.max(2, ring.maxR * 0.18 * (1 - f * 0.85) * ring.intensity)
    // Brightness decays with age but stays high early for punch.
    const brightness = clamp(ring.intensity * (1 - f * 0.7) * intensityScale, 0, 1)

    const hue = phase + ring.hueOffset + 0.32
    ctx.strokeStyle = samplePalette(pal, hue, brightness)
    ctx.lineWidth = width
    ctx.beginPath()
    ctx.arc(ring.x, ring.y, radius, 0, TAU)
    ctx.stroke()
}

function drawCore(
    ctx: CanvasRenderingContext2D,
    cx: number,
    cy: number,
    baseRadius: number,
    pal: readonly Rgb[],
    phase: number,
    bass: number,
    pulse: number,
    rawPulse: number,
    intensityScale: number,
    hueBase: number,
): void {
    // Halo ring — a thin stroked circle that blooms outward on beats.
    // Uses rawPulse so it snaps on transients instead of sluggishly smoothing.
    const haloR = baseRadius * (2.5 + rawPulse * 3.6 + bass * 1.3)
    const haloW = Math.max(2.5, baseRadius * 0.22)
    const haloBrightness = clamp((0.5 + rawPulse * 0.45) * intensityScale, 0, 1)
    ctx.strokeStyle = samplePalette(pal, hueBase + phase + 0.4, haloBrightness)
    ctx.lineWidth = haloW
    ctx.beginPath()
    ctx.arc(cx, cy, haloR, 0, TAU)
    ctx.stroke()

    // Outer core disk — big, bass- and pulse-driven.
    const outer = baseRadius * (1.15 + bass * 2.3 + pulse * 1.7)
    const outerBrightness = clamp((0.72 + pulse * 0.28) * intensityScale, 0, 1)
    ctx.fillStyle = samplePalette(pal, hueBase + phase + 0.22, outerBrightness)
    ctx.beginPath()
    ctx.arc(cx, cy, outer, 0, TAU)
    ctx.fill()

    // Dark mid ring — creates a contrast band inside the outer disk.
    const mid = outer * (0.6 + rawPulse * 0.15)
    const midBrightness = clamp((0.45 + bass * 0.18) * intensityScale, 0, 1)
    ctx.fillStyle = samplePalette(pal, hueBase + phase, midBrightness)
    ctx.beginPath()
    ctx.arc(cx, cy, mid, 0, TAU)
    ctx.fill()

    // Bright inner disk — the nucleus, brightest palette accent.
    const inner = Math.max(3, outer * (0.32 + rawPulse * 0.14))
    const innerBrightness = clamp((0.96 + rawPulse * 0.04) * intensityScale, 0, 1)
    ctx.fillStyle = samplePalette(pal, hueBase + phase + 0.5, innerBrightness)
    ctx.beginPath()
    ctx.arc(cx, cy, inner, 0, TAU)
    ctx.fill()

    // Tiny bright nucleus dot — pulses with audio but doesn't rotate.
    // Reads as a steady heartbeat without competing for attention.
    const accentR = inner * (0.5 + rawPulse * 0.28 + bass * 0.18)
    ctx.fillStyle = samplePalette(pal, hueBase + phase + 0.1, clamp(0.98 * intensityScale, 0, 1))
    ctx.beginPath()
    ctx.arc(cx, cy, accentR, 0, TAU)
    ctx.fill()
}

function spawnBurst(
    emitterIndex: number,
    analysis: AudioState,
    speed: number,
    intensity: number,
    decay: number,
): Burst {
    const life = 0.55 + (1 - decay) * 1.0
    const speedFactor = 1.15 + speed * 0.14 + decay * 0.8
    return {
        age: 0,
        emitterIndex,
        hueShift: randomBetween(-0.45, 0.45),
        life,
        speed: speedFactor,
        strength: clamp(0.72 + analysis.pulse * 0.7 + intensity * 0.22, 0.55, 1.7),
    }
}

function spawnFlashRing(
    x: number,
    y: number,
    maxR: number,
    analysis: AudioState,
    speed: number,
    intensity: number,
    emitterIndex: number,
): FlashRing {
    return {
        age: 0,
        emitterIndex,
        hueOffset: randomBetween(-0.12, 0.18),
        intensity: clamp(0.55 + analysis.pulse * 0.55 + intensity * 0.25, 0.4, 1.4),
        life: 0.45 - speed * 0.018,
        maxR,
        x,
        y,
    }
}

export default canvas.stateful(
    'Shockwave',
    {
        palette: combo('Palette', ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit'], {
            default: 'SilkCircuit',
            group: 'Scene',
        }),
        scene: combo('Scene', [...SCENES], { default: 'Core Burst', group: 'Scene' }),
        speed: num('Speed', [1, 10], 6, { group: 'Motion' }),
        intensity: num('Intensity', [0, 100], 78, { group: 'Motion' }),
        decay: num('Decay', [0, 100], 52, { group: 'Motion' }),
        ringCount: num('Ring Count', [2, 12], 6, { group: 'Geometry' }),
    },
    () => {
        let phase = 0
        let rotation = 0
        // Counter-rotating ray rotation — spins opposite to the dark mandala wedges
        // so the spoke-pair interaction reads as two-gear machinery.
        let rayRotation = 0
        // Continuous flow time — cascades palette colors outward through rings.
        let flowTime = 0
        let synthClock = 0
        let bursts: Burst[] = []
        let flashRings: FlashRing[] = []
        let lastTime = -1
        let beatHeld = false
        let nextEmitter = 0
        // Center jolt: beats kick it off-center, damped spring pulls it back.
        let joltX = 0
        let joltY = 0
        let joltVX = 0
        let joltVY = 0
        // Continuous bass accumulator — sustained bass leaks micro-bursts.
        let bassAccum = 0
        // Smoothed audio envelopes — asymmetric attack/release so the visuals
        // pump cleanly without snapping on transients.
        let smBass = 0
        let smTreble = 0
        let smLevel = 0
        let smPulse = 0
        let smFieldScale = 1

        return (ctx, time, controls) => {
            const speed = controls.speed as number
            const intensity = clamp((controls.intensity as number) / 100, 0, 1)
            const decay = clamp((controls.decay as number) / 100, 0, 1)
            const ringCount = Math.max(2, Math.round(controls.ringCount as number))
            const scene = resolveSceneName(controls.scene)
            const paletteName = resolvePaletteName(controls.palette)
            const pal = LED_PALETTES[paletteName]

            const w = ctx.canvas.width
            const h = ctx.canvas.height
            const dt = lastTime < 0 ? 1 / 60 : Math.min(0.05, time - lastTime)
            lastTime = time

            synthClock += dt * (0.8 + speed * 0.15)
            const analysis = analyzeAudio(audio(), synthClock)

            // Asymmetric smoothing: fast attack (catches transients), slow release
            // (holds visual energy between hits). Prevents snap/flicker.
            const smoothAttackRelease = (cur: number, target: number, atk: number, rel: number): number => {
                const lambda = target > cur ? atk : rel
                const factor = 1 - Math.exp(-lambda * dt)
                return cur + (target - cur) * factor
            }
            smBass = smoothAttackRelease(smBass, analysis.bass, 14, 3.2)
            smTreble = smoothAttackRelease(smTreble, analysis.treble, 12, 4)
            smLevel = smoothAttackRelease(smLevel, analysis.level, 9, 3)
            smPulse = smoothAttackRelease(smPulse, analysis.pulse, 22, 2.4)

            // Continuous drifts keep the image alive and flowing even in silence.
            phase += dt * (0.045 + speed * 0.022 + smBass * 0.28)
            rotation += dt * (0.18 + smTreble * 1.6 + speed * 0.05 + smPulse * 0.75)
            // Rays spin opposite, faster on bass — two-gear rotor feel.
            rayRotation -= dt * (0.32 + smBass * 2.4 + speed * 0.08 + smPulse * 0.5)
            flowTime += dt * (0.12 + speed * 0.035 + smLevel * 0.5 + smBass * 0.55)

            // Bass accumulator: when sustained bass is present, drip extra bursts
            // so the field stays in motion between clean transients.
            bassAccum += smBass * dt * (0.8 + speed * 0.12)

            // Age existing entities and prune dead ones.
            for (const b of bursts) b.age += dt * b.speed
            bursts = bursts.filter((b) => b.age < b.life)
            for (const f of flashRings) f.age += dt
            flashRings = flashRings.filter((f) => f.age < f.life)

            // Determine emitter positions up-front so flash rings can target them.
            const joltCx = w * 0.5 + joltX
            const joltCy = h * 0.5 + joltY
            // Ambient orbit amplified by audio — quiet music barely drifts,
            // loud music swings the whole composition around.
            const ambientAmp = 1 + smLevel * 3.0 + smBass * 1.6
            const wobbleX = Math.sin(time * 0.23) * w * 0.013 * ambientAmp
            const wobbleY = Math.cos(time * 0.31) * h * 0.013 * ambientAmp
            // Bass-driven vertical sway — the field breathes heavier on bass.
            const bassSway = Math.sin(time * 1.08) * smBass * h * 0.022
            const wobbleYTotal = wobbleY + bassSway

            let centersA: [number, number]
            let centersB: [number, number]
            let fieldMaxR: number

            if (scene === 'Twin Burst') {
                // Closer together so the two fields merge into one coherent shockwave
                // with two bright cores — not two discrete "eyes" sitting apart.
                const off = w * 0.17
                centersA = [joltCx - off + wobbleX, joltCy + wobbleYTotal]
                centersB = [joltCx + off + wobbleX, joltCy + wobbleYTotal]
                fieldMaxR = Math.min(w * 0.82, Math.sqrt(w * w + h * h) * 0.64)
            } else {
                centersA = [joltCx + wobbleX, joltCy + wobbleYTotal]
                centersB = centersA
                fieldMaxR = Math.sqrt(w * w + h * h) * 0.62
            }

            // Spawn bursts + flash rings on transients.
            if (analysis.spawnNow && !beatHeld) {
                beatHeld = true
                if (scene === 'Twin Burst') {
                    bursts.push(spawnBurst(nextEmitter, analysis, speed, intensity, decay))
                    const side = nextEmitter === 0 ? centersA : centersB
                    flashRings.push(spawnFlashRing(side[0], side[1], fieldMaxR, analysis, speed, intensity, nextEmitter))
                    nextEmitter = 1 - nextEmitter
                    if (analysis.pulse > 0.55) {
                        bursts.push(spawnBurst(1 - nextEmitter, analysis, speed, intensity, decay))
                        const other = nextEmitter === 0 ? centersB : centersA
                        flashRings.push(
                            spawnFlashRing(other[0], other[1], fieldMaxR, analysis, speed, intensity, 1 - nextEmitter),
                        )
                    }
                } else {
                    bursts.push(spawnBurst(-1, analysis, speed, intensity, decay))
                    flashRings.push(
                        spawnFlashRing(centersA[0], centersA[1], fieldMaxR, analysis, speed, intensity, -1),
                    )
                }
                // Cap active entities.
                if (bursts.length > ringCount * 2 + 6) {
                    bursts = bursts.slice(bursts.length - (ringCount * 2 + 6))
                }
                if (flashRings.length > 6) {
                    flashRings = flashRings.slice(flashRings.length - 6)
                }
                // Jolt: kick the center, spring it back. Bigger impulse so beats
                // are visibly felt as a lurch in the whole composition.
                const joltAngle = randomBetween(0, TAU)
                const joltMag = (20 + analysis.pulse * 55 + intensity * 22) * (scene === 'Twin Burst' ? 0.45 : 1)
                joltVX += Math.cos(joltAngle) * joltMag
                joltVY += Math.sin(joltAngle) * joltMag
            } else if (analysis.pulse < 0.12) {
                beatHeld = false
            }

            // Micro-bursts from sustained bass — drips every time accumulator crosses threshold.
            const bassThreshold = 0.55
            while (bassAccum >= bassThreshold) {
                bassAccum -= bassThreshold
                // Only spawn if we're not already saturated.
                if (bursts.length < ringCount * 2 + 6) {
                    const em = scene === 'Twin Burst' ? (bursts.length % 2) : -1
                    bursts.push(spawnBurst(em, analysis, speed, intensity * 0.6, decay))
                }
            }

            // Damped spring returns the center to origin. Slightly under-damped
            // so a jolt lands, rebounds once, then settles — more kinetic.
            const k = 58
            const c = 8.5
            joltVX += (-k * joltX - c * joltVX) * dt
            joltVY += (-k * joltY - c * joltVY) * dt
            joltX += joltVX * dt
            joltY += joltVY * dt

            // Background: darkest palette stop. Never pure black — LEDs always see tone.
            const ground = pal[0]
            ctx.fillStyle = rgbCss(ground)
            ctx.fillRect(0, 0, w, h)

            const intensityScale = 0.65 + intensity * 0.45

            // Bass inflates the entire field — smoothed so it blooms rather than snaps.
            const targetFieldScale = 1 + smBass * 0.24 + smPulse * 0.2
            smFieldScale = smoothAttackRelease(smFieldScale, targetFieldScale, 9, 4)
            const fieldScale = smFieldScale

            // Smoothed audio snapshot for rendering — keeps ellipticity and brightness
            // pumping cleanly without visible steps on transients.
            const renderAudio: AudioState = {
                bass: smBass,
                level: smLevel,
                onset: analysis.onset,
                present: analysis.present,
                pulse: smPulse,
                spawnNow: analysis.spawnNow,
                treble: smTreble,
            }
            // Raw pulse passed through for punchy elements (core halo, inner disk, rays).
            // Snapping on transients is the point — smoothing kills the feel.
            const rawPulse = analysis.pulse

            if (scene === 'Twin Burst') {
                const coreR = Math.min(w, h) * 0.055

                // Interleave ring-by-ring so both cores stay visible even with heavy overlap.
                for (let i = ringCount; i >= 0; i -= 1) {
                    drawBullseyeRing({
                        audio: renderAudio,
                        bursts,
                        ctx,
                        cx: centersA[0],
                        cy: centersA[1],
                        emitterIndex: 0,
                        fieldScale,
                        flowTime,
                        hueBase: 0,
                        intensityScale,
                        maxR: fieldMaxR,
                        pal,
                        phase,
                        ringCount,
                        ringIndex: i,
                        rotationBase: rotation,
                        time,
                    })
                    drawBullseyeRing({
                        audio: renderAudio,
                        bursts,
                        ctx,
                        cx: centersB[0],
                        cy: centersB[1],
                        emitterIndex: 1,
                        fieldScale,
                        flowTime,
                        hueBase: 0.32,
                        intensityScale,
                        maxR: fieldMaxR,
                        pal,
                        phase,
                        ringCount,
                        ringIndex: i,
                        rotationBase: -rotation * 0.85,
                        time,
                    })
                }

                for (const fr of flashRings) drawFlashRing(ctx, fr, pal, phase, intensityScale)

                drawCore(
                    ctx,
                    centersA[0],
                    centersA[1],
                    coreR,
                    pal,
                    phase,
                    smBass,
                    smPulse,
                    rawPulse,
                    intensityScale,
                    0,
                )
                drawCore(
                    ctx,
                    centersB[0],
                    centersB[1],
                    coreR,
                    pal,
                    phase,
                    smBass,
                    smPulse,
                    rawPulse,
                    intensityScale,
                    0.32,
                )
            } else {
                const coreR = Math.min(w, h) * 0.06

                drawBullseye({
                    audio: analysis,
                    bursts,
                    ctx,
                    cx: centersA[0],
                    cy: centersA[1],
                    emitterIndex: 0,
                    fieldScale,
                    flowTime,
                    hueBase: 0,
                    intensityScale,
                    maxR: fieldMaxR,
                    pal,
                    phase,
                    ringCount,
                    rotationBase: rotation,
                    time,
                })

                if (scene === 'Cascade') {
                    const spokes = Math.round(3 + ringCount * 0.42)
                    // Wedge width pumps with the beat — pinwheel tightens then splays on hits.
                    const wedgeFrac = 0.12 + smPulse * 0.06
                    drawMandalaCuts(
                        ctx, centersA[0], centersA[1], fieldMaxR, spokes,
                        rotation, rgbCss(ground), wedgeFrac, time, smBass,
                    )
                    // Two-tier ray system: primary rays lockstep with the beat (read as
                    // the pulse), accent rays weave between them at independent rates.
                    drawBrightRays(
                        ctx, centersA[0], centersA[1], fieldMaxR, spokes,
                        rayRotation, -rayRotation * 0.6,
                        pal, phase, time, smPulse, rawPulse, smBass, smTreble, intensityScale,
                    )
                }

                for (const fr of flashRings) drawFlashRing(ctx, fr, pal, phase, intensityScale)

                drawCore(
                    ctx,
                    centersA[0],
                    centersA[1],
                    coreR,
                    pal,
                    phase,
                    smBass,
                    smPulse,
                    rawPulse,
                    intensityScale,
                    0,
                )
            }
        }
    },
    {
        audio: true,
        description:
            'Concentric shockwaves flow outward through rotating banded fields. Bass inflates the whole pattern, beats fire bright pressure rings racing across the canvas, and the ambient swirl keeps things moving between hits.',
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
                    'Stand at ground zero of a tectonic rupture. Dense fire-colored rings swirl outward and every impact drives a white-hot pressure ring racing through them.',
                name: 'Seismic Epicenter',
            },
            {
                controls: {
                    decay: 75,
                    intensity: 60,
                    palette: 'Ice',
                    ringCount: 5,
                    scene: 'Cascade',
                    speed: 3,
                },
                description:
                    'An ice shelf fractures in slow motion. Wide cold bands wheel behind a glacial mandala while sparse pressure waves travel outward like frozen thunder.',
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
                    'Two containment fields collide. Twin shockwave cores merge into a single neon corridor as alternating pressure rings fire between the rupture points.',
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
                    'The northern lights break apart like stained glass. Soft green and violet bands drift behind a slow polar mandala, pressure waves pulsing through the cold.',
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
                    'A digital bomb detonates inside a circuit board. Maximum ring density, electric purple and cyan stacked tight, pressure rings firing in rapid succession.',
                name: 'SilkCircuit Detonation',
            },
            {
                controls: {
                    decay: 55,
                    intensity: 82,
                    palette: 'Ice',
                    ringCount: 8,
                    scene: 'Core Burst',
                    speed: 2,
                },
                description:
                    'The edge of a collapsing star. Broad glacial rings hang suspended as slow shockwaves radiate outward — every pulse a gravitational echo.',
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
                    'Molten geometry spins relentlessly through the dark. Every beat hammers another pressure wave across the pinwheel of burning bands.',
                name: 'Forge Hammer',
            },
        ],
    },
)
