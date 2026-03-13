import { canvas, combo, num } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface Cell {
    cx: number
    cy: number
    seed: number
    phase: number
}

interface Rgb { r: number; g: number; b: number }

// ── Constants ────────────────────────────────────────────────────────────

const SCENES = ['Lattice', 'Prism', 'Shardfield', 'Signal']
const TAU = Math.PI * 2

// ── Helpers ──────────────────────────────────────────────────────────────

function clamp(v: number, lo: number, hi: number): number {
    return Math.max(lo, Math.min(hi, v))
}

function hash(n: number): number {
    const x = Math.sin(n * 127.1 + 311.7) * 43758.5453
    return x - Math.floor(x)
}

function hash2(a: number, b: number): number {
    return hash(a * 73.13 + b * 113.97)
}

function hsvToRgb(h: number, s: number, v: number): Rgb {
    const hue = ((h % 360) + 360) % 360
    const sat = clamp(s, 0, 1)
    const val = clamp(v, 0, 1)
    const c = val * sat
    const x = c * (1 - Math.abs(((hue / 60) % 2) - 1))
    const m = val - c

    let r = 0
    let g = 0
    let b = 0
    if (hue < 60) { r = c; g = x }
    else if (hue < 120) { r = x; g = c }
    else if (hue < 180) { g = c; b = x }
    else if (hue < 240) { g = x; b = c }
    else if (hue < 300) { r = x; b = c }
    else { r = c; b = x }

    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    }
}

function toRgba(c: Rgb, a: number): string {
    return `rgba(${c.r},${c.g},${c.b},${clamp(a, 0, 1).toFixed(3)})`
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const r = clamp(t, 0, 1)
    return {
        r: Math.round(a.r + (b.r - a.r) * r),
        g: Math.round(a.g + (b.g - a.g) * r),
        b: Math.round(a.b + (b.b - a.b) * r),
    }
}

function easeSigned(value: number, exponent: number): number {
    const sign = Math.sign(value)
    return sign * Math.abs(value) ** exponent
}

// ── LED-Safe Palettes ────────────────────────────────────────────────────
// Tier 1/2 hues (180-330 safe vivid range), high saturation, controlled value.

interface FrostPalette {
    bg: Rgb
    primary: Rgb
    accent: Rgb
    highlight: Rgb
}

const PALETTES: Record<string, FrostPalette> = {
    SilkCircuit: {
        bg:        { r: 5, g: 3, b: 18 },
        primary:   hsvToRgb(285, 0.92, 0.92),   // electric purple
        accent:    hsvToRgb(180, 0.90, 0.88),    // cyan
        highlight: hsvToRgb(320, 0.85, 0.95),    // hot pink
    },
    Ice: {
        bg:        { r: 3, g: 8, b: 20 },
        primary:   hsvToRgb(200, 0.88, 0.90),    // azure
        accent:    hsvToRgb(190, 0.85, 0.85),    // cool cyan
        highlight: hsvToRgb(220, 0.80, 0.95),    // bright blue
    },
    Frost: {
        bg:        { r: 4, g: 10, b: 22 },
        primary:   hsvToRgb(210, 0.85, 0.88),    // deep azure
        accent:    hsvToRgb(185, 0.90, 0.82),    // teal
        highlight: hsvToRgb(240, 0.78, 0.92),    // blue
    },
    Aurora: {
        bg:        { r: 4, g: 6, b: 16 },
        primary:   hsvToRgb(150, 0.88, 0.85),    // spring green
        accent:    hsvToRgb(270, 0.85, 0.88),    // purple
        highlight: hsvToRgb(180, 0.90, 0.92),    // cyan
    },
    Cyberpunk: {
        bg:        { r: 6, g: 2, b: 12 },
        primary:   hsvToRgb(300, 0.92, 0.95),    // magenta
        accent:    hsvToRgb(180, 0.88, 0.90),    // cyan
        highlight: hsvToRgb(330, 0.85, 0.92),    // rose
    },
}

function getPalette(name: string): FrostPalette {
    return PALETTES[name] ?? PALETTES.Ice
}

// ── Hex Grid Geometry ────────────────────────────────────────────────────

function hexCenter(col: number, row: number, size: number): [number, number] {
    const x = size * 1.5 * col
    const y = size * Math.sqrt(3) * (row + (col % 2) * 0.5)
    return [x, y]
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful('Frost Crystal', {
    speed:    [1, 10, 5],
    scale:    [10, 100, 46],
    edgeGlow: [0, 100, 68],
    growth:   [0, 100, 68],
    rotation: num('Rotation', [-100, 100], 0, {
        tooltip: 'Spin the crystal field. Negative values reverse the direction.',
    }),
    palette:  combo('Palette', ['Aurora', 'Cyberpunk', 'Frost', 'Ice', 'SilkCircuit'], { default: 'Ice' }),
    scene:    SCENES,
}, () => {
    let cells: Cell[] = []
    let lastCellKey = ''

    function buildCells(w: number, h: number, hexSize: number): void {
        const key = `${w}:${h}:${Math.round(hexSize)}`
        if (key === lastCellKey) return
        lastCellKey = key

        cells = []
        const span = Math.hypot(w, h)
        const cols = Math.ceil(span / (hexSize * 1.5)) + 4
        const rows = Math.ceil(span / (hexSize * Math.sqrt(3))) + 4
        const xOffset = w * 0.5 - span * 0.5
        const yOffset = h * 0.5 - span * 0.5

        for (let c = -2; c < cols; c++) {
            for (let r = -2; r < rows; r++) {
                const [baseX, baseY] = hexCenter(c, r, hexSize)
                const seed = hash2(c + 100, r + 100)
                cells.push({
                    cx: xOffset + baseX,
                    cy: yOffset + baseY,
                    seed,
                    phase: seed * TAU,
                })
            }
        }
    }

    return (ctx, time, c) => {
        const speed = c.speed as number
        const scaleMix = clamp((c.scale as number) / 100, 0, 1)
        const growthMix = clamp((c.growth as number) / 100, 0, 1)
        const glowMix = clamp((c.edgeGlow as number) / 100, 0, 1)
        const rotationMix = clamp((c.rotation as number) / 100, -1, 1)
        const paletteName = c.palette as string
        const scene = c.scene as string

        const pal = getPalette(paletteName)
        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const t = time * (0.3 + speed * 0.2)
        const rotationAngle = easeSigned(rotationMix, 1.25) * time * 0.85

        // Hex cell size: 18-55px — large enough for LED visibility
        const hexSize = 18 + (1 - scaleMix) * 37

        buildCells(w, h, hexSize)

        // Clear with dark background
        ctx.fillStyle = `rgb(${pal.bg.r},${pal.bg.g},${pal.bg.b})`
        ctx.fillRect(0, 0, w, h)

        ctx.save()
        ctx.translate(w * 0.5, h * 0.5)
        ctx.rotate(rotationAngle)
        ctx.translate(-w * 0.5, -h * 0.5)
        ctx.globalCompositeOperation = 'lighter'

        // Growth wave — radial sweep from center
        const growthRadius = (w + h) * 0.5
        const growthWave = (t * 0.15 * (1 + growthMix * 0.8)) % 1.0
        const growthCenter = { x: w * 0.5, y: h * 0.5 }

        for (const cell of cells) {
            const distFromCenter = Math.hypot(
                cell.cx - growthCenter.x,
                cell.cy - growthCenter.y,
            )
            const normalizedDist = distFromCenter / growthRadius

            // Growth activation — cells light up in waves from center
            const wavePhase = (growthWave - normalizedDist * 0.6 + cell.seed * 0.15 + 1) % 1
            const activation = clamp(
                Math.sin(wavePhase * Math.PI) * (1.2 + growthMix * 0.8),
                0, 1,
            )

            if (activation < 0.03) continue

            // Breathing pulse per cell — sinusoidal, 2-4s cycle
            const breathe = 0.6 + 0.4 * Math.sin(
                t * (1.2 + cell.seed * 0.8) + cell.phase,
            )

            const cellAlpha = activation * breathe

            // Choose scene-specific cell rendering
            if (scene === 'Lattice' || scene === 'Prism') {
                drawHexCell(
                    ctx, cell.cx, cell.cy, hexSize * 0.9,
                    pal, cellAlpha, glowMix, scene === 'Prism',
                )
            } else if (scene === 'Shardfield') {
                drawShardCell(
                    ctx, cell.cx, cell.cy, hexSize * 0.9,
                    pal, cellAlpha, glowMix, cell.seed, t,
                )
            } else {
                drawSignalCell(
                    ctx, cell.cx, cell.cy, hexSize * 0.9,
                    pal, cellAlpha, glowMix, cell.seed, t,
                )
            }
        }

        // Node highlights at cell centers — hot-spot technique
        for (const cell of cells) {
            const distFromCenter = Math.hypot(
                cell.cx - growthCenter.x,
                cell.cy - growthCenter.y,
            )
            const normalizedDist = distFromCenter / growthRadius
            const wavePhase = (growthWave - normalizedDist * 0.6 + cell.seed * 0.15 + 1) % 1
            const activation = clamp(Math.sin(wavePhase * Math.PI) * 1.4, 0, 1)

            if (activation < 0.2) continue

            const sparkle = 
                Math.max(0, Math.sin(t * (3 + cell.seed * 2) + cell.phase * 3)) ** 
                4
            
            const nodeAlpha = activation * sparkle * (0.3 + glowMix * 0.5)

            if (nodeAlpha < 0.04) continue

            const nodeSize = 3 + glowMix * 4
            const grad = ctx.createRadialGradient(
                cell.cx, cell.cy, 0,
                cell.cx, cell.cy, nodeSize,
            )
            grad.addColorStop(0, toRgba(pal.highlight, nodeAlpha * 0.9))
            grad.addColorStop(0.5, toRgba(pal.accent, nodeAlpha * 0.3))
            grad.addColorStop(1, 'rgba(0,0,0,0)')
            ctx.fillStyle = grad
            ctx.beginPath()
            ctx.arc(cell.cx, cell.cy, nodeSize, 0, TAU)
            ctx.fill()
        }

        ctx.restore()
    }

    // ── Cell Renderers ───────────────────────────────────────────────

    function drawHexCell(
        ctx: CanvasRenderingContext2D,
        cx: number, cy: number, size: number,
        pal: FrostPalette, alpha: number, glow: number,
        isPrism: boolean,
    ): void {
        // Bold hex outline — wide strokes for LED visibility
        const edgeWidth = 3 + glow * 4

        // Inner fill — dim colored, creating darkness contrast
        const fillGrad = ctx.createRadialGradient(cx, cy, 0, cx, cy, size)
        fillGrad.addColorStop(0, toRgba(pal.primary, alpha * 0.08))
        fillGrad.addColorStop(0.7, toRgba(pal.accent, alpha * 0.04))
        fillGrad.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = fillGrad
        ctx.beginPath()
        drawHexPath(ctx, cx, cy, size)
        ctx.fill()

        // Bold edges — primary color
        ctx.lineWidth = edgeWidth
        ctx.strokeStyle = toRgba(pal.primary, alpha * (0.4 + glow * 0.4))
        ctx.beginPath()
        drawHexPath(ctx, cx, cy, size)
        ctx.stroke()

        if (isPrism) {
            // Inner concentric hex — accent color
            ctx.lineWidth = edgeWidth * 0.7
            ctx.strokeStyle = toRgba(pal.accent, alpha * (0.25 + glow * 0.3))
            ctx.beginPath()
            drawHexPath(ctx, cx, cy, size * 0.55)
            ctx.stroke()
        }
    }

    function drawShardCell(
        ctx: CanvasRenderingContext2D,
        cx: number, cy: number, size: number,
        pal: FrostPalette, alpha: number, glow: number,
        seed: number, t: number,
    ): void {
        // Diamond / shard shape — rotated square
        const angle = seed * Math.PI + t * 0.1
        const shardSize = size * (0.5 + seed * 0.3)

        // Fill with gradient
        const grad = ctx.createRadialGradient(cx, cy, 0, cx, cy, shardSize)
        grad.addColorStop(0, toRgba(pal.accent, alpha * 0.12))
        grad.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = grad
        ctx.beginPath()
        drawDiamondPath(ctx, cx, cy, shardSize, angle)
        ctx.fill()

        // Bold shard edges
        const edgeWidth = 3 + glow * 3.5
        ctx.lineWidth = edgeWidth
        ctx.strokeStyle = toRgba(
            mixRgb(pal.primary, pal.accent, seed),
            alpha * (0.35 + glow * 0.45),
        )
        ctx.beginPath()
        drawDiamondPath(ctx, cx, cy, shardSize, angle)
        ctx.stroke()
    }

    function drawSignalCell(
        ctx: CanvasRenderingContext2D,
        cx: number, cy: number, size: number,
        pal: FrostPalette, alpha: number, glow: number,
        seed: number, t: number,
    ): void {
        // Concentric rings — expanding ripple pattern per cell
        const numRings = 2 + Math.floor(seed * 2)
        const edgeWidth = 2.5 + glow * 3

        for (let i = 0; i < numRings; i++) {
            const ringPhase = (t * 0.3 + seed * 2 + i * 0.3) % 1
            const ringRadius = size * (0.2 + ringPhase * 0.7)
            const ringAlpha = alpha * (1 - ringPhase) * (0.3 + glow * 0.4)

            if (ringAlpha < 0.03) continue

            const color = i === 0 ? pal.primary : (i === 1 ? pal.accent : pal.highlight)
            ctx.lineWidth = edgeWidth * (1 - ringPhase * 0.5)
            ctx.strokeStyle = toRgba(color, ringAlpha)
            ctx.beginPath()
            ctx.arc(cx, cy, ringRadius, 0, TAU)
            ctx.stroke()
        }
    }

    function drawHexPath(
        ctx: CanvasRenderingContext2D,
        cx: number, cy: number, size: number,
    ): void {
        for (let i = 0; i < 6; i++) {
            const angle = (Math.PI / 3) * i - Math.PI / 6
            const x = cx + Math.cos(angle) * size
            const y = cy + Math.sin(angle) * size
            if (i === 0) ctx.moveTo(x, y)
            else ctx.lineTo(x, y)
        }
        ctx.closePath()
    }

    function drawDiamondPath(
        ctx: CanvasRenderingContext2D,
        cx: number, cy: number, size: number,
        angle: number,
    ): void {
        for (let i = 0; i < 4; i++) {
            const a = angle + (Math.PI / 2) * i
            const x = cx + Math.cos(a) * size
            const y = cy + Math.sin(a) * size
            if (i === 0) ctx.moveTo(x, y)
            else ctx.lineTo(x, y)
        }
        ctx.closePath()
    }
}, {
    description: 'Bold crystalline hex lattice with frost-growth waves, breathing nodes, and field rotation',
    presets: [
        {
            name: 'Frozen Tundra at Dawn',
            description: 'First light creeping across permafrost — vast hexagonal ice plates slowly pulsing with pale blue fire, the field barely turning like a frozen compass',
            controls: {
                speed: 2,
                scale: 28,
                edgeGlow: 80,
                growth: 45,
                rotation: 8,
                palette: 'Ice',
                scene: 'Lattice',
            },
        },
        {
            name: 'Crystal Cave Bioluminescence',
            description: 'Deep underground where quartz meets living light — prismatic hexagons breathing in aurora greens and purples, rippling outward from an invisible heart',
            controls: {
                speed: 4,
                scale: 52,
                edgeGlow: 72,
                growth: 85,
                rotation: -15,
                palette: 'Aurora',
                scene: 'Prism',
            },
        },
        {
            name: 'Obsidian Shatter',
            description: 'Volcanic glass exploding under pressure — sharp diamond shards spinning fast in hot magenta and cyan, edges crackling with electric discharge',
            controls: {
                speed: 9,
                scale: 75,
                edgeGlow: 95,
                growth: 92,
                rotation: -68,
                palette: 'Cyberpunk',
                scene: 'Shardfield',
            },
        },
        {
            name: 'Frost Mandala',
            description: 'Sacred geometry forming on a windowpane at absolute zero — concentric signal rings rippling through deep azure frost, spinning with meditative precision',
            controls: {
                speed: 3,
                scale: 40,
                edgeGlow: 60,
                growth: 55,
                rotation: 32,
                palette: 'Frost',
                scene: 'Signal',
            },
        },
        {
            name: 'SilkCircuit Lattice',
            description: 'A motherboard dreaming in crystalline geometry — electric purple hex cells breathing with neon nodes, the entire field rotating like a silicon prayer wheel',
            controls: {
                speed: 6,
                scale: 58,
                edgeGlow: 88,
                growth: 72,
                rotation: 42,
                palette: 'SilkCircuit',
                scene: 'Prism',
            },
        },
    ],
})
