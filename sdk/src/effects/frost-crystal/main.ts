import { canvas, clamp, combo, num } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface Cell {
    cx: number
    cy: number
    seed: number
    phase: number
}

interface Rgb {
    r: number
    g: number
    b: number
}

// ── Constants ────────────────────────────────────────────────────────────

const SCENES = ['Lattice', 'Prism', 'Shardfield', 'Signal', 'Dendrite', 'Koch', 'Interference']
const TAU = Math.PI * 2

// ── Helpers ──────────────────────────────────────────────────────────────

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
        b: Math.round((b + m) * 255),
        g: Math.round((g + m) * 255),
        r: Math.round((r + m) * 255),
    }
}

function toRgba(c: Rgb, a: number): string {
    return `rgba(${c.r},${c.g},${c.b},${clamp(a, 0, 1).toFixed(3)})`
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const r = clamp(t, 0, 1)
    return {
        b: Math.round(a.b + (b.b - a.b) * r),
        g: Math.round(a.g + (b.g - a.g) * r),
        r: Math.round(a.r + (b.r - a.r) * r),
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
    Aurora: {
        accent: hsvToRgb(270, 0.85, 0.88), // purple
        bg: { b: 16, g: 6, r: 4 },
        highlight: hsvToRgb(180, 0.9, 0.92), // cyan
        primary: hsvToRgb(150, 0.88, 0.85), // spring green
    },
    Cyberpunk: {
        accent: hsvToRgb(180, 0.88, 0.9), // cyan
        bg: { b: 12, g: 2, r: 6 },
        highlight: hsvToRgb(330, 0.85, 0.92), // rose
        primary: hsvToRgb(300, 0.92, 0.95), // magenta
    },
    Frost: {
        accent: hsvToRgb(185, 0.9, 0.82), // teal
        bg: { b: 22, g: 10, r: 4 },
        highlight: hsvToRgb(240, 0.78, 0.92), // blue
        primary: hsvToRgb(210, 0.85, 0.88), // deep azure
    },
    Ice: {
        accent: hsvToRgb(190, 0.85, 0.85), // cool cyan
        bg: { b: 20, g: 8, r: 3 },
        highlight: hsvToRgb(220, 0.8, 0.95), // bright blue
        primary: hsvToRgb(200, 0.88, 0.9), // azure
    },
    SilkCircuit: {
        accent: hsvToRgb(180, 0.9, 0.88), // cyan
        bg: { b: 18, g: 3, r: 5 },
        highlight: hsvToRgb(320, 0.85, 0.95), // hot pink
        primary: hsvToRgb(285, 0.92, 0.92), // electric purple
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

// ── Koch Snowflake Geometry ──────────────────────────────────────────

const KOCH_COS_N60 = 0.5
const KOCH_SIN_N60 = -Math.sqrt(3) / 2

function kochSubdivide(points: [number, number][], depth: number): [number, number][] {
    let edges = points
    for (let d = 0; d < depth; d++) {
        const next: [number, number][] = []
        for (let i = 0; i < edges.length; i++) {
            const [ax, ay] = edges[i]
            const [bx, by] = edges[(i + 1) % edges.length]
            const dx = bx - ax
            const dy = by - ay
            const p1x = ax + dx / 3
            const p1y = ay + dy / 3
            const p2x = ax + (dx * 2) / 3
            const p2y = ay + (dy * 2) / 3
            // Peak: rotate (p2-p1) by -60° around p1
            const sdx = p2x - p1x
            const sdy = p2y - p1y
            const peakX = p1x + sdx * KOCH_COS_N60 - sdy * KOCH_SIN_N60
            const peakY = p1y + sdx * KOCH_SIN_N60 + sdy * KOCH_COS_N60
            next.push([ax, ay], [p1x, p1y], [peakX, peakY], [p2x, p2y])
        }
        edges = next
    }
    return edges
}

function kochSnowflake(cx: number, cy: number, radius: number, depth: number, rotation: number): [number, number][] {
    const triangle: [number, number][] = []
    for (let i = 0; i < 3; i++) {
        const angle = rotation + (TAU / 3) * i - Math.PI / 2
        triangle.push([cx + Math.cos(angle) * radius, cy + Math.sin(angle) * radius])
    }
    return kochSubdivide(triangle, depth)
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful(
    'Frost Crystal',
    {
        palette: combo('Palette', ['Aurora', 'Cyberpunk', 'Frost', 'Ice', 'SilkCircuit'], {
            default: 'Ice',
            group: 'Scene',
        }),
        scene: combo('Scene', SCENES, { default: 'Lattice', group: 'Scene' }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        rotation: num('Rotation', [-100, 100], 0, {
            tooltip: 'Spin the crystal field. Negative values reverse the direction.',
            group: 'Motion',
        }),
        scale: num('Scale', [10, 100], 46, { group: 'Geometry' }),
        edgeGlow: num('Edge Glow', [0, 100], 68, { group: 'Geometry' }),
        growth: num('Growth', [0, 100], 68, { group: 'Geometry' }),
    },
    () => {
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
                        phase: seed * TAU,
                        seed,
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
                const distFromCenter = Math.hypot(cell.cx - growthCenter.x, cell.cy - growthCenter.y)
                const normalizedDist = distFromCenter / growthRadius

                // Growth activation — cells light up in waves from center
                const wavePhase = (growthWave - normalizedDist * 0.6 + cell.seed * 0.15 + 1) % 1
                const activation = clamp(Math.sin(wavePhase * Math.PI) * (1.2 + growthMix * 0.8), 0, 1)

                if (activation < 0.03) continue

                // Breathing pulse per cell — sinusoidal, 2-4s cycle
                const breathe = 0.6 + 0.4 * Math.sin(t * (1.2 + cell.seed * 0.8) + cell.phase)

                const cellAlpha = activation * breathe

                // Choose scene-specific cell rendering
                const cellSize = hexSize * 0.9
                if (scene === 'Lattice' || scene === 'Prism') {
                    drawHexCell(ctx, cell.cx, cell.cy, cellSize, pal, cellAlpha, glowMix, scene === 'Prism')
                } else if (scene === 'Shardfield') {
                    drawShardCell(ctx, cell.cx, cell.cy, cellSize, pal, cellAlpha, glowMix, cell.seed, t)
                } else if (scene === 'Dendrite') {
                    drawDendriteCell(ctx, cell.cx, cell.cy, cellSize, pal, cellAlpha, glowMix, cell.seed, t)
                } else if (scene === 'Koch') {
                    drawKochCell(ctx, cell.cx, cell.cy, cellSize, pal, cellAlpha, glowMix, cell.seed, t)
                } else if (scene === 'Interference') {
                    drawInterferenceCell(ctx, cell.cx, cell.cy, cellSize, pal, cellAlpha, glowMix, cell.seed, t)
                } else {
                    drawSignalCell(ctx, cell.cx, cell.cy, cellSize, pal, cellAlpha, glowMix, cell.seed, t)
                }
            }

            // Node highlights at cell centers — hot-spot technique
            for (const cell of cells) {
                const distFromCenter = Math.hypot(cell.cx - growthCenter.x, cell.cy - growthCenter.y)
                const normalizedDist = distFromCenter / growthRadius
                const wavePhase = (growthWave - normalizedDist * 0.6 + cell.seed * 0.15 + 1) % 1
                const activation = clamp(Math.sin(wavePhase * Math.PI) * 1.4, 0, 1)

                if (activation < 0.2) continue

                const sparkle = Math.max(0, Math.sin(t * (3 + cell.seed * 2) + cell.phase * 3)) ** 4

                const nodeAlpha = activation * sparkle * (0.3 + glowMix * 0.5)

                if (nodeAlpha < 0.04) continue

                const nodeSize = 3 + glowMix * 4
                const grad = ctx.createRadialGradient(cell.cx, cell.cy, 0, cell.cx, cell.cy, nodeSize)
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
            cx: number,
            cy: number,
            size: number,
            pal: FrostPalette,
            alpha: number,
            glow: number,
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
            cx: number,
            cy: number,
            size: number,
            pal: FrostPalette,
            alpha: number,
            glow: number,
            seed: number,
            t: number,
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
            ctx.strokeStyle = toRgba(mixRgb(pal.primary, pal.accent, seed), alpha * (0.35 + glow * 0.45))
            ctx.beginPath()
            drawDiamondPath(ctx, cx, cy, shardSize, angle)
            ctx.stroke()
        }

        function drawSignalCell(
            ctx: CanvasRenderingContext2D,
            cx: number,
            cy: number,
            size: number,
            pal: FrostPalette,
            alpha: number,
            glow: number,
            seed: number,
            t: number,
        ): void {
            // Concentric rings — expanding ripple pattern per cell
            const numRings = 2 + Math.floor(seed * 2)
            const edgeWidth = 2.5 + glow * 3

            for (let i = 0; i < numRings; i++) {
                const ringPhase = (t * 0.3 + seed * 2 + i * 0.3) % 1
                const ringRadius = size * (0.2 + ringPhase * 0.7)
                const ringAlpha = alpha * (1 - ringPhase) * (0.3 + glow * 0.4)

                if (ringAlpha < 0.03) continue

                const color = i === 0 ? pal.primary : i === 1 ? pal.accent : pal.highlight
                ctx.lineWidth = edgeWidth * (1 - ringPhase * 0.5)
                ctx.strokeStyle = toRgba(color, ringAlpha)
                ctx.beginPath()
                ctx.arc(cx, cy, ringRadius, 0, TAU)
                ctx.stroke()
            }
        }

        function drawHexPath(ctx: CanvasRenderingContext2D, cx: number, cy: number, size: number): void {
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
            cx: number,
            cy: number,
            size: number,
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

        // ── Dendrite (recursive 6-fold branching) ─────────────────────

        function drawDendriteBranch(
            ctx: CanvasRenderingContext2D,
            x: number,
            y: number,
            angle: number,
            length: number,
            width: number,
            pal: FrostPalette,
            alpha: number,
            glow: number,
            seed: number,
            t: number,
            depth: number,
            maxDepth: number,
        ): void {
            if (depth > maxDepth || length < 2) return

            // Sinusoidal breathing — each depth level phase-shifted
            const breathe = 0.82 + 0.18 * Math.sin(t * (0.8 + seed * 0.4) + depth * 1.5)
            const len = length * breathe
            const endX = x + Math.cos(angle) * len
            const endY = y + Math.sin(angle) * len

            // Color gradient through palette by depth
            const depthT = depth / maxDepth
            const color =
                depth === 0
                    ? pal.primary
                    : depth === 1
                      ? mixRgb(pal.primary, pal.accent, 0.5)
                      : mixRgb(pal.accent, pal.highlight, depthT)

            ctx.lineWidth = width * (1 - depthT * 0.4)
            ctx.strokeStyle = toRgba(color, alpha * (1 - depthT * 0.3) * (0.4 + glow * 0.45))
            ctx.lineCap = 'round'
            ctx.beginPath()
            ctx.moveTo(x, y)
            ctx.lineTo(endX, endY)
            ctx.stroke()

            // Sub-branches at ±60° — ice crystal growth directions
            if (depth < maxDepth) {
                const subLength = len * (0.48 + hash(seed * 17 + depth * 31) * 0.12)
                const subWidth = width * 0.6
                const branchOffset = Math.PI / 3
                const frac = 0.55 + hash(seed * 23 + depth * 7) * 0.1
                const bx = x + Math.cos(angle) * len * frac
                const by = y + Math.sin(angle) * len * frac

                drawDendriteBranch(
                    ctx,
                    bx,
                    by,
                    angle + branchOffset,
                    subLength,
                    subWidth,
                    pal,
                    alpha,
                    glow,
                    seed,
                    t,
                    depth + 1,
                    maxDepth,
                )
                drawDendriteBranch(
                    ctx,
                    bx,
                    by,
                    angle - branchOffset,
                    subLength,
                    subWidth,
                    pal,
                    alpha,
                    glow,
                    seed,
                    t,
                    depth + 1,
                    maxDepth,
                )
            }
        }

        function drawDendriteCell(
            ctx: CanvasRenderingContext2D,
            cx: number,
            cy: number,
            size: number,
            pal: FrostPalette,
            alpha: number,
            glow: number,
            seed: number,
            t: number,
        ): void {
            const armLength = size * 0.85
            const baseWidth = 2.5 + glow * 3.5

            for (let arm = 0; arm < 6; arm++) {
                const baseAngle = (TAU / 6) * arm + seed * 0.2
                drawDendriteBranch(ctx, cx, cy, baseAngle, armLength, baseWidth, pal, alpha, glow, seed, t, 0, 3)
            }
        }

        // ── Koch Snowflake (fractal cell outlines) ────────────────────

        function drawKochCell(
            ctx: CanvasRenderingContext2D,
            cx: number,
            cy: number,
            size: number,
            pal: FrostPalette,
            alpha: number,
            glow: number,
            seed: number,
            t: number,
        ): void {
            const rotation = t * 0.08 + seed * TAU
            const points = kochSnowflake(cx, cy, size * 0.88, 2, rotation)
            const edgeWidth = 2.5 + glow * 3.5

            // Radial fill — dim interior
            const grad = ctx.createRadialGradient(cx, cy, 0, cx, cy, size)
            grad.addColorStop(0, toRgba(pal.primary, alpha * 0.1))
            grad.addColorStop(0.6, toRgba(pal.accent, alpha * 0.05))
            grad.addColorStop(1, 'rgba(0,0,0,0)')
            ctx.fillStyle = grad
            ctx.beginPath()
            ctx.moveTo(points[0][0], points[0][1])
            for (let i = 1; i < points.length; i++) {
                ctx.lineTo(points[i][0], points[i][1])
            }
            ctx.closePath()
            ctx.fill()

            // Fractal outline — bold for LED readability
            const colorT = Math.sin(t * 0.3 + seed * TAU) * 0.5 + 0.5
            ctx.lineWidth = edgeWidth
            ctx.strokeStyle = toRgba(mixRgb(pal.primary, pal.highlight, colorT), alpha * (0.35 + glow * 0.45))
            ctx.lineJoin = 'round'
            ctx.beginPath()
            ctx.moveTo(points[0][0], points[0][1])
            for (let i = 1; i < points.length; i++) {
                ctx.lineTo(points[i][0], points[i][1])
            }
            ctx.closePath()
            ctx.stroke()

            // Inner snowflake — offset 30°, depth 1, accent color
            if (glow > 0.3) {
                const innerPts = kochSnowflake(cx, cy, size * 0.45, 1, rotation + Math.PI / 6)
                ctx.lineWidth = edgeWidth * 0.6
                ctx.strokeStyle = toRgba(pal.accent, alpha * (glow - 0.3) * 0.5)
                ctx.beginPath()
                ctx.moveTo(innerPts[0][0], innerPts[0][1])
                for (let i = 1; i < innerPts.length; i++) {
                    ctx.lineTo(innerPts[i][0], innerPts[i][1])
                }
                ctx.closePath()
                ctx.stroke()
            }
        }

        // ── Interference (hexagonal wave superposition) ───────────────

        function drawInterferenceCell(
            ctx: CanvasRenderingContext2D,
            cx: number,
            cy: number,
            size: number,
            pal: FrostPalette,
            alpha: number,
            glow: number,
            seed: number,
            t: number,
        ): void {
            // Dense coherent wave rings with 6-fold angular modulation
            const numRings = 5 + Math.floor(glow * 5)
            const wavelength = size * 0.32
            const edgeWidth = 1.8 + glow * 2.2
            const angularMod = 0.1 + glow * 0.1
            const phaseShift = t * 1.2 + seed * TAU
            const STEPS = 36

            for (let i = 0; i < numRings; i++) {
                const baseRadius = wavelength * (i + 0.5) + Math.sin(phaseShift + i * 0.5) * wavelength * 0.15
                const ringAlpha = alpha * (0.12 + glow * 0.18) * (1 - (i / numRings) * 0.6)

                if (ringAlpha < 0.02) continue

                const colorT = (i / numRings + seed + t * 0.02) % 1
                const color = mixRgb(pal.primary, pal.accent, colorT)

                ctx.lineWidth = edgeWidth * (1 - (i / numRings) * 0.3)
                ctx.strokeStyle = toRgba(color, ringAlpha)
                ctx.beginPath()

                // Angular modulation → hexagonal wave fronts
                for (let s = 0; s <= STEPS; s++) {
                    const angle = (s / STEPS) * TAU
                    const modulation = 1 + angularMod * Math.cos(angle * 6 + seed * TAU + t * 0.3)
                    const r = baseRadius * modulation
                    const px = cx + Math.cos(angle) * r
                    const py = cy + Math.sin(angle) * r
                    if (s === 0) ctx.moveTo(px, py)
                    else ctx.lineTo(px, py)
                }
                ctx.closePath()
                ctx.stroke()
            }
        }
    },
    {
        description:
            'Crystalline hex lattice propagates in frost-growth waves — nodes breathe with cold light as the frozen field slowly rotates',
        presets: [
            {
                controls: {
                    edgeGlow: 80,
                    growth: 45,
                    palette: 'Ice',
                    rotation: 8,
                    scale: 28,
                    scene: 'Lattice',
                    speed: 2,
                },
                description:
                    'First light creeping across permafrost — vast hexagonal ice plates slowly pulsing with pale blue fire, the field barely turning like a frozen compass',
                name: 'Frozen Tundra at Dawn',
            },
            {
                controls: {
                    edgeGlow: 72,
                    growth: 85,
                    palette: 'Aurora',
                    rotation: -15,
                    scale: 52,
                    scene: 'Prism',
                    speed: 4,
                },
                description:
                    'Deep underground where quartz meets living light — prismatic hexagons breathing in aurora greens and purples, rippling outward from an invisible heart',
                name: 'Crystal Cave Bioluminescence',
            },
            {
                controls: {
                    edgeGlow: 95,
                    growth: 92,
                    palette: 'Cyberpunk',
                    rotation: -68,
                    scale: 75,
                    scene: 'Shardfield',
                    speed: 9,
                },
                description:
                    'Volcanic glass exploding under pressure — sharp diamond shards spinning fast in hot magenta and cyan, edges crackling with electric discharge',
                name: 'Obsidian Shatter',
            },
            {
                controls: {
                    edgeGlow: 60,
                    growth: 55,
                    palette: 'Frost',
                    rotation: 32,
                    scale: 40,
                    scene: 'Signal',
                    speed: 3,
                },
                description:
                    'Sacred geometry forming on a windowpane at absolute zero — concentric signal rings rippling through deep azure frost, spinning with meditative precision',
                name: 'Frost Mandala',
            },
            {
                controls: {
                    edgeGlow: 88,
                    growth: 72,
                    palette: 'SilkCircuit',
                    rotation: 42,
                    scale: 58,
                    scene: 'Prism',
                    speed: 6,
                },
                description:
                    'A motherboard dreaming in crystalline geometry — electric purple hex cells breathing with neon nodes, the entire field rotating like a silicon prayer wheel',
                name: 'SilkCircuit Lattice',
            },
            {
                controls: {
                    edgeGlow: 40,
                    growth: 100,
                    palette: 'Aurora',
                    rotation: 0,
                    scale: 10,
                    scene: 'Lattice',
                    speed: 1,
                },
                description:
                    'A continent of microscopic ice crystals forms on a telescope lens aimed at the aurora — infinite tiny hexagons pulse with captured starlight',
                name: 'Polar Microscope',
            },
            {
                controls: {
                    edgeGlow: 100,
                    growth: 30,
                    palette: 'Cyberpunk',
                    rotation: -100,
                    scale: 90,
                    scene: 'Signal',
                    speed: 10,
                },
                description:
                    'A rogue satellite broadcasts distress rings in hot magenta while spinning out of control — signal pulses scream into the void',
                name: 'Rogue Satellite',
            },
            {
                controls: {
                    edgeGlow: 55,
                    growth: 80,
                    palette: 'Frost',
                    rotation: 18,
                    scale: 62,
                    scene: 'Shardfield',
                    speed: 4,
                },
                description:
                    'Stained glass shatters in slow motion inside a collapsing ice cathedral — azure diamond shards tumble through pale blue fog',
                name: 'Ice Cathedral Collapse',
            },
            {
                controls: {
                    edgeGlow: 75,
                    growth: 70,
                    palette: 'Ice',
                    rotation: 5,
                    scale: 35,
                    scene: 'Dendrite',
                    speed: 3,
                },
                description:
                    'Microscopic ice crystals bloom in perfect hexagonal symmetry — six-armed dendrites branch and breathe with pale blue phosphorescence',
                name: 'Snowflake Garden',
            },
            {
                controls: {
                    edgeGlow: 90,
                    growth: 85,
                    palette: 'Aurora',
                    rotation: -12,
                    scale: 55,
                    scene: 'Dendrite',
                    speed: 5,
                },
                description:
                    'Crystalline ferns unfurl in aurora light — recursive branches split and split again, each generation glowing a deeper green-violet',
                name: 'Crystalline Bloom',
            },
            {
                controls: {
                    edgeGlow: 82,
                    growth: 65,
                    palette: 'Frost',
                    rotation: 10,
                    scale: 30,
                    scene: 'Koch',
                    speed: 2,
                },
                description:
                    'Infinite fractal snowflakes tessellate in frozen geometry — Koch curves trace impossible coastlines of ice, each edge spawning smaller copies of itself',
                name: 'Fractal Permafrost',
            },
            {
                controls: {
                    edgeGlow: 95,
                    growth: 90,
                    palette: 'Cyberpunk',
                    rotation: -55,
                    scale: 70,
                    scene: 'Koch',
                    speed: 8,
                },
                description:
                    'Neon snowflakes spin like throwing stars in a cyberpunk blizzard — hot magenta fractals with cyan cores rotating through electric darkness',
                name: 'Neon Snowflake Dojo',
            },
            {
                controls: {
                    edgeGlow: 70,
                    growth: 60,
                    palette: 'Frost',
                    rotation: 0,
                    scale: 42,
                    scene: 'Interference',
                    speed: 3,
                },
                description:
                    'Hexagonal wave fronts ripple outward from crystal nodes, overlapping into standing wave patterns — frozen sound made visible in deep azure',
                name: 'Standing Waves',
            },
            {
                controls: {
                    edgeGlow: 88,
                    growth: 78,
                    palette: 'SilkCircuit',
                    rotation: 25,
                    scale: 50,
                    scene: 'Interference',
                    speed: 6,
                },
                description:
                    'Electric purple wave sources pulse in crystalline symmetry — where hexagonal ripples collide, moiré ghosts shimmer in neon cyan',
                name: 'Crystal Resonance',
            },
        ],
    },
)
