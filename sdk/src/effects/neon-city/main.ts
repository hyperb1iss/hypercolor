import { canvas, combo } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface Star {
    x: number
    y: number
    size: number
    drift: number
    twinkle: number
}

interface Shard {
    x: number
    y: number
    length: number
    width: number
    baseAngle: number
    spin: number
    hueOffset: number
}

interface Spark {
    lane: number
    offset: number
    speed: number
    size: number
    jitter: number
}

interface PaletteSet {
    bgA: string
    bgB: string
    star: string
    shardA: string
    shardB: string
    sweep: string
    spark: string
}

// ── Constants ────────────────────────────────────────────────────────────

const COLOR_MODES = ['SilkCircuit', 'Dark Matter', 'Ion Storm', 'Supernova', 'Aurora'] as const
const SCENES = ['Orbital', 'Rift', 'Hyperlane']

// ── Helpers ──────────────────────────────────────────────────────────────

function hexToRgb(hex: string): { r: number; g: number; b: number } {
    const norm = hex.replace('#', '')
    const full = norm.length === 3
        ? `${norm[0]}${norm[0]}${norm[1]}${norm[1]}${norm[2]}${norm[2]}`
        : norm
    const n = parseInt(full, 16)
    return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 }
}

function hexToRgba(hex: string, alpha: number): string {
    const c = hexToRgb(hex)
    return `rgba(${c.r}, ${c.g}, ${c.b}, ${Math.max(0, Math.min(1, alpha)).toFixed(3)})`
}

function hash(v: number): number {
    const s = Math.sin(v * 12.9898 + 78.233) * 43758.5453
    return s - Math.floor(s)
}

function getPalette(name: string): PaletteSet {
    if (name === 'SilkCircuit') {
        return {
            bgA: '#0f0621', bgB: '#1f1238', star: '#80ffea',
            shardA: '#e135ff', shardB: '#ff6ac1', sweep: '#80ffea', spark: '#f1fa8c',
        }
    }
    if (name === 'Ion Storm') {
        return {
            bgA: '#051126', bgB: '#0a2d45', star: '#9be5ff',
            shardA: '#5ad8ff', shardB: '#b8f6ff', sweep: '#62c9ff', spark: '#f3ffb8',
        }
    }
    if (name === 'Supernova') {
        return {
            bgA: '#270e05', bgB: '#44210c', star: '#ffd9a8',
            shardA: '#ff7c2a', shardB: '#ffd16c', sweep: '#ff9545', spark: '#fff2b5',
        }
    }
    if (name === 'Aurora') {
        return {
            bgA: '#071c1a', bgB: '#122731', star: '#c9fff4',
            shardA: '#43ff95', shardB: '#ad7bff', sweep: '#85ffd8', spark: '#e8fff1',
        }
    }
    // Dark Matter (default)
    return {
        bgA: '#06091a', bgB: '#130f2a', star: '#b8d1ff',
        shardA: '#8a5bff', shardB: '#ff57d6', sweep: '#82a8ff', spark: '#d8f2ff',
    }
}

function getVoidCenter(scene: number, w: number, h: number, time: number): { x: number; y: number } {
    if (scene === 1) {
        return {
            x: w * 0.30 + Math.sin(time * 0.5) * 12,
            y: h * 0.50 + Math.cos(time * 0.4) * 8,
        }
    }
    if (scene === 2) {
        return {
            x: w * 0.66 + Math.cos(time * 0.55) * 10,
            y: h * 0.42 + Math.sin(time * 0.35) * 7,
        }
    }
    return {
        x: w * 0.50 + Math.sin(time * 0.4) * 9,
        y: h * 0.50 + Math.cos(time * 0.3) * 9,
    }
}

function computeCounts(density: number): { stars: number; shards: number; sparks: number } {
    const d = Math.max(0, Math.min(100, density)) / 100
    return {
        stars: Math.floor(70 + d * 170),
        shards: Math.floor(24 + d * 84),
        sparks: Math.floor(12 + d * 48),
    }
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful('Neon City', {
    speed:        [1, 10, 5],
    shardDensity: [10, 100, 58],
    lineSweep:    [0, 100, 62],
    voidStrength: [0, 100, 56],
    sparks:       true,
    glow:         [0, 100, 72],
    colorMode:    combo('Palette', [...COLOR_MODES], { default: 'Dark Matter' }),
    scene:        SCENES,
}, () => {
    let stars: Star[] = []
    let shards: Shard[] = []
    let sparkLanes: Spark[] = []
    let counts = { stars: 130, shards: 52, sparks: 24 }

    let prevShardDensity = 58

    function seedFields(w: number, h: number): void {
        stars = Array.from({ length: counts.stars }, () => ({
            x: Math.random(),
            y: Math.random(),
            size: 0.6 + Math.random() * 1.8,
            drift: 0.05 + Math.random() * 0.35,
            twinkle: Math.random() * Math.PI * 2,
        }))

        shards = Array.from({ length: counts.shards }, () => ({
            x: Math.random() * w,
            y: Math.random() * h,
            length: 8 + Math.random() * 42,
            width: 1 + Math.random() * 2.8,
            baseAngle: Math.random() * Math.PI * 2,
            spin: (Math.random() - 0.5) * 1.6,
            hueOffset: Math.random(),
        }))

        sparkLanes = Array.from({ length: counts.sparks }, () => ({
            lane: Math.random(),
            offset: Math.random(),
            speed: 0.2 + Math.random() * 0.9,
            size: 1 + Math.random() * 2.2,
            jitter: Math.random() * Math.PI * 2,
        }))
    }

    function drawBackground(
        ctx: CanvasRenderingContext2D, w: number, h: number,
        palette: PaletteSet, time: number,
    ): void {
        const grad = ctx.createLinearGradient(0, 0, 0, h)
        grad.addColorStop(0, palette.bgA)
        grad.addColorStop(1, palette.bgB)
        ctx.fillStyle = grad
        ctx.fillRect(0, 0, w, h)

        const haze = ctx.createRadialGradient(w * 0.45, h * 0.3, 12, w * 0.45, h * 0.3, Math.max(w, h) * 0.8)
        haze.addColorStop(0, hexToRgba(palette.shardA, 0.22))
        haze.addColorStop(1, hexToRgba(palette.shardA, 0.0))
        ctx.fillStyle = haze
        ctx.fillRect(0, 0, w, h)

        const grainCount = 70
        for (let i = 0; i < grainCount; i++) {
            const px = hash(i * 1.37 + 4.9 + time * 0.2) * w
            const py = hash(i * 2.11 + 3.4 + time * 0.15) * h
            const alpha = 0.02 + hash(i * 4.21 + 1.7) * 0.04
            ctx.fillStyle = hexToRgba('#ffffff', alpha)
            ctx.fillRect(px, py, 1, 1)
        }
    }

    function drawVoid(
        ctx: CanvasRenderingContext2D, x: number, y: number,
        minDim: number, strength: number, palette: PaletteSet, time: number,
    ): void {
        const r = minDim * (0.12 + strength * 0.24)
        const ringR = r * (1.25 + 0.15 * Math.sin(time * 0.8))

        const core = ctx.createRadialGradient(x, y, 0, x, y, r)
        core.addColorStop(0, hexToRgba('#000000', 0.92))
        core.addColorStop(1, hexToRgba('#000000', 0.0))
        ctx.fillStyle = core
        ctx.beginPath()
        ctx.arc(x, y, r, 0, Math.PI * 2)
        ctx.fill()

        ctx.strokeStyle = hexToRgba(palette.shardB, 0.20 + strength * 0.32)
        ctx.lineWidth = 2.5
        ctx.beginPath()
        ctx.arc(x, y, ringR, 0, Math.PI * 2)
        ctx.stroke()
    }

    function drawStars(
        ctx: CanvasRenderingContext2D, w: number, h: number,
        time: number, palette: PaletteSet, glow: number,
    ): void {
        for (const s of stars) {
            const tw = 0.35 + 0.65 * (0.5 + 0.5 * Math.sin(time * (0.8 + s.drift) + s.twinkle))
            const x = (s.x * w + Math.sin(time * s.drift + s.twinkle) * 3 + w) % w
            const y = (s.y * h + Math.cos(time * s.drift * 0.7 + s.twinkle) * 2 + h) % h
            const radius = s.size * (0.8 + tw * 0.7)

            ctx.fillStyle = hexToRgba(palette.star, 0.24 + tw * 0.7)
            ctx.beginPath()
            ctx.arc(x, y, radius, 0, Math.PI * 2)
            ctx.fill()

            if (glow > 0.2 && tw > 0.65) {
                ctx.fillStyle = hexToRgba(palette.star, 0.08 + glow * 0.20)
                ctx.beginPath()
                ctx.arc(x, y, radius * 3.2, 0, Math.PI * 2)
                ctx.fill()
            }
        }
    }

    function drawShards(
        ctx: CanvasRenderingContext2D, w: number, h: number,
        time: number, palette: PaletteSet, glow: number, sceneIdx: number,
    ): void {
        const driftX = sceneIdx === 2 ? -16 : 10
        const driftY = sceneIdx === 1 ? 8 : -4

        for (const shard of shards) {
            const x = (shard.x + Math.sin(time * 0.3 + shard.hueOffset * 8) * driftX + w) % w
            const y = (shard.y + Math.cos(time * 0.22 + shard.hueOffset * 7) * driftY + h) % h
            const a = shard.baseAngle + time * shard.spin

            const x1 = x + Math.cos(a) * shard.length * 0.5
            const y1 = y + Math.sin(a) * shard.length * 0.5
            const x2 = x - Math.cos(a) * shard.length * 0.5
            const y2 = y - Math.sin(a) * shard.length * 0.5

            const c = shard.hueOffset > 0.5 ? palette.shardA : palette.shardB

            ctx.strokeStyle = hexToRgba(c, 0.28 + glow * 0.52)
            ctx.lineWidth = shard.width
            ctx.lineCap = 'round'
            ctx.beginPath()
            ctx.moveTo(x1, y1)
            ctx.lineTo(x2, y2)
            ctx.stroke()

            ctx.strokeStyle = hexToRgba(palette.sweep, 0.05 + glow * 0.10)
            ctx.lineWidth = Math.max(1, shard.width * 0.45)
            ctx.beginPath()
            ctx.moveTo(x1 * 0.96 + x * 0.04, y1 * 0.96 + y * 0.04)
            ctx.lineTo(x2 * 0.96 + x * 0.04, y2 * 0.96 + y * 0.04)
            ctx.stroke()
        }
    }

    function drawSweepLines(
        ctx: CanvasRenderingContext2D, w: number, h: number,
        time: number, palette: PaletteSet, sweep: number, sceneIdx: number,
    ): void {
        if (sweep <= 0.01) return

        const laneCount = 4 + Math.floor(sweep * 10)
        const tilt = sceneIdx === 1 ? 0.6 : sceneIdx === 2 ? -0.45 : -0.22
        const speed = 22 + sweep * 80

        ctx.lineWidth = 1.25
        for (let i = 0; i < laneCount; i++) {
            const offset = ((time * speed + i * (h / laneCount + 16)) % (h + 40)) - 20
            const wobble = Math.sin(time * 1.4 + i * 2.1) * 9

            ctx.strokeStyle = hexToRgba(palette.sweep, 0.08 + sweep * 0.18)
            ctx.beginPath()
            ctx.moveTo(-20, offset + wobble)
            ctx.lineTo(w + 20, offset + wobble + tilt * w)
            ctx.stroke()
        }
    }

    function drawSparks(
        ctx: CanvasRenderingContext2D, w: number, h: number,
        time: number, palette: PaletteSet, glow: number, sceneIdx: number,
    ): void {
        const dir = sceneIdx === 1 ? -1 : 1
        for (const s of sparkLanes) {
            const x = ((s.offset + time * s.speed * 0.25 * dir) % 1 + 1) % 1
            const laneY = h * (0.1 + s.lane * 0.82)
            const y = laneY + Math.sin(time * 3 + s.jitter) * (3 + sceneIdx * 1.5)
            const px = x * w

            const head = 2 + s.size * 1.4
            const tail = 12 + s.size * 10

            ctx.strokeStyle = hexToRgba(palette.spark, 0.34 + glow * 0.46)
            ctx.lineWidth = s.size
            ctx.beginPath()
            ctx.moveTo(px - tail * dir, y)
            ctx.lineTo(px, y)
            ctx.stroke()

            ctx.fillStyle = hexToRgba(palette.spark, 0.32 + glow * 0.28)
            ctx.beginPath()
            ctx.arc(px, y, head, 0, Math.PI * 2)
            ctx.fill()
        }
    }

    // Initialize on first frame
    let initialized = false

    return (ctx, time, c) => {
        const speed = c.speed as number
        const shardDensity = c.shardDensity as number
        const lineSweep = c.lineSweep as number
        const voidStrength = c.voidStrength as number
        const sparksEnabled = c.sparks as boolean
        const glowCtrl = c.glow as number
        const colorMode = c.colorMode as string
        const sceneName = c.scene as string
        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const t = time * speed

        // Detect density changes
        if (shardDensity !== prevShardDensity || !initialized) {
            const nextCounts = computeCounts(shardDensity)
            if (
                nextCounts.stars !== counts.stars ||
                nextCounts.shards !== counts.shards ||
                nextCounts.sparks !== counts.sparks ||
                !initialized
            ) {
                counts = nextCounts
                seedFields(w, h)
            }
            prevShardDensity = shardDensity
            initialized = true
        }

        const palette = getPalette(colorMode)
        const glow = glowCtrl / 100
        const sweep = lineSweep / 100
        const voidStr = voidStrength / 100

        drawBackground(ctx, w, h, palette, t)

        const sceneIndex = SCENES.indexOf(sceneName)
        const center = getVoidCenter(sceneIndex, w, h, t)
        drawVoid(ctx, center.x, center.y, Math.min(w, h), voidStr, palette, t)

        drawStars(ctx, w, h, t, palette, glow)
        drawShards(ctx, w, h, t, palette, glow, sceneIndex)
        drawSweepLines(ctx, w, h, t, palette, sweep, sceneIndex)

        if (sparksEnabled) {
            drawSparks(ctx, w, h, t, palette, glow, sceneIndex)
        }
    }
}, {
    description: 'Geometric electric-space composition with stars, shards, sweep lines, and a dark-matter void',
})
