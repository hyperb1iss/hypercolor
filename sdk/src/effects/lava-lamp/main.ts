import { canvas, color, combo, num, toggle } from '@hypercolor/sdk'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface Rgb {
    r: number
    g: number
    b: number
}
interface ThemePalette {
    color1: string
    color2: string
    color3: string
}

interface Ball {
    x: number
    y: number
    vx: number
    vy: number
    size: number
}

interface GridPoint {
    x: number
    y: number
    force: number
    computed: number
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const THEMES = ['Aurora', 'Bubblegum', 'Citrus', 'Custom', 'Lagoon', 'Molten', 'Synthwave', 'Toxic']

const THEME_PALETTES: Record<string, ThemePalette> = {
    Aurora: { color1: '#33f587', color2: '#3fdcff', color3: '#8c4bff' },
    Bubblegum: { color1: '#ff4f9a', color2: '#ff74c5', color3: '#8a5cff' },
    Citrus: { color1: '#ffb347', color2: '#ff7a2f', color3: '#ff5778' },
    Custom: { color1: '#16d1d9', color2: '#ff4fb4', color3: '#7d49ff' },
    Lagoon: { color1: '#3cf2df', color2: '#4a96ff', color3: '#163dff' },
    Molten: { color1: '#ff6329', color2: '#ff8d1f', color3: '#ff4b5c' },
    Synthwave: { color1: '#ff4ed6', color2: '#8f48ff', color3: '#42d9ff' },
    Toxic: { color1: '#36ff9a', color2: '#0ae0cb', color3: '#6c2bff' },
}

const STEP = 5

// Marching squares lookup tables (ge1doot algorithm)
const MSCASES = [0, 3, 0, 3, 1, 3, 0, 3, 2, 2, 0, 2, 1, 1, 0]
const PLX = [0, 0, 1, 0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0]
const PLY = [0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1]
const IX = [1, 0, -1, 0, 0, 1, 0, -1, -1, 0, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1]

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

function hexToRgb(hex: string): Rgb {
    const h = hex.replace('#', '')
    const f = h.length === 3 ? `${h[0]}${h[0]}${h[1]}${h[1]}${h[2]}${h[2]}` : h
    const v = Number.parseInt(f, 16)
    return { b: v & 255, g: (v >> 8) & 255, r: (v >> 16) & 255 }
}

function rgbToHsl(c: Rgb): [number, number, number] {
    const r = c.r / 255
    const g = c.g / 255
    const b = c.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const d = max - min
    const l = (max + min) * 0.5
    if (d === 0) return [0, 0, l]
    const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
    let h = 0
    if (max === r) h = (g - b) / d + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / d + 2
    else h = (r - g) / d + 4
    return [h * 60, s, l]
}

function hslCss(h: number, s: number, l: number): string {
    return `hsl(${h % 360}, ${Math.round(s * 100)}%, ${Math.round(l * 100)}%)`
}

function resolvePalette(theme: string, c1: string, c2: string, c3: string): ThemePalette {
    if (theme !== 'Custom') return THEME_PALETTES[theme] ?? THEME_PALETTES.Custom
    return { color1: c1, color2: c2, color3: c3 }
}

// ---------------------------------------------------------------------------
// Ball physics — simple bouncing, smooth and predictable
// ---------------------------------------------------------------------------

function createBalls(count: number, w: number, h: number): Ball[] {
    const wh = Math.min(w, h)
    const balls: Ball[] = []
    for (let i = 0; i < count; i++) {
        balls.push({
            size: wh / 15 + (Math.random() * 1.4 + 0.1) * (wh / 15),
            vx: (Math.random() > 0.5 ? 1 : -1) * (0.2 + Math.random() * 0.25),
            vy: (Math.random() > 0.5 ? 1 : -1) * (0.2 + Math.random()),
            x: w * 0.2 + Math.random() * w * 0.6,
            y: h * 0.2 + Math.random() * h * 0.6,
        })
    }
    return balls
}

function moveBalls(balls: Ball[], speed: number, w: number, h: number): void {
    const spd = speed / 25
    for (const b of balls) {
        if (b.x >= w - b.size) {
            if (b.vx > 0) b.vx = -b.vx
            b.x = w - b.size
        } else if (b.x <= b.size) {
            if (b.vx < 0) b.vx = -b.vx
            b.x = b.size
        }
        if (b.y >= h - b.size) {
            if (b.vy > 0) b.vy = -b.vy
            b.y = h - b.size
        } else if (b.y <= b.size) {
            if (b.vy < 0) b.vy = -b.vy
            b.y = b.size
        }
        b.x += b.vx * spd
        b.y += b.vy * spd
    }
}

// ---------------------------------------------------------------------------
// Grid construction
// ---------------------------------------------------------------------------

function createGrid(sx: number, sy: number): GridPoint[] {
    const cols = sx + 2
    const total = cols * (sy + 2)
    const grid: GridPoint[] = new Array(total)
    for (let i = 0; i < total; i++) {
        grid[i] = {
            computed: 0,
            force: 0,
            x: (i % cols) * STEP,
            y: Math.floor(i / cols) * STEP,
        }
    }
    return grid
}

// ---------------------------------------------------------------------------
// Metaball renderer — marching squares contour tracing with gradient fill
// ---------------------------------------------------------------------------

function renderMetaballs(
    ctx: CanvasRenderingContext2D,
    grid: GridPoint[],
    sx: number,
    sy: number,
    balls: Ball[],
    fill: CanvasGradient,
    state: { iter: number; sign: number },
): void {
    const cols = sx + 2
    state.iter++
    state.sign = -state.sign

    function computeForce(x: number, y: number, idx: number): number {
        if (x === 0 || y === 0 || x === sx || y === sy) {
            grid[idx].force = 0.6 * state.sign
            return grid[idx].force
        }
        const cell = grid[idx]
        let force = 0
        for (const ball of balls) {
            const dx = cell.x - ball.x
            const dy = cell.y - ball.y
            force += (ball.size * ball.size) / (dx * dx + dy * dy)
        }
        force *= state.sign
        grid[idx].force = force
        return force
    }

    function ensureForce(x: number, y: number, idx: number): number {
        const f = grid[idx].force
        if ((f > 0 && state.sign < 0) || (f < 0 && state.sign > 0) || !f) {
            return computeForce(x, y, idx)
        }
        return f
    }

    ctx.fillStyle = fill
    ctx.beginPath()
    let paint = false

    for (const ball of balls) {
        let x = Math.round(ball.x / STEP)
        let y = Math.round(ball.y / STEP)
        let pdir: number | false = false
        let safety = (sx + 2) * (sy + 2)

        while (safety-- > 0) {
            const id = x + y * cols
            if (id < 0 || id >= grid.length || grid[id].computed === state.iter) break

            let mscase = 0
            for (let i = 0; i < 4; i++) {
                const nx = x + IX[i + 12]
                const ny = y + IX[i + 16]
                const nid = nx + ny * cols
                if (nid >= 0 && nid < grid.length && Math.abs(ensureForce(nx, ny, nid)) > 1) {
                    mscase += 1 << i
                }
            }

            if (mscase === 15) {
                // Fully inside the blob — move up to find the edge
                y--
                pdir = false
                continue
            }

            let dir: number
            if (mscase === 5) dir = pdir === 2 ? 3 : 1
            else if (mscase === 10) dir = pdir === 3 ? 0 : 2
            else {
                dir = MSCASES[mscase]
                grid[id].computed = state.iter
            }

            // Interpolated contour point
            const p1 = grid[x + PLX[4 * dir + 2] + (y + PLY[4 * dir + 2]) * cols]
            const p2 = grid[x + PLX[4 * dir + 3] + (y + PLY[4 * dir + 3]) * cols]
            const ratio = Math.abs(Math.abs(p1.force) - 1) / Math.abs(Math.abs(p2.force) - 1)
            const interp = STEP / (ratio + 1)

            const base = grid[x + PLX[4 * dir] + (y + PLY[4 * dir]) * cols]
            const base2 = grid[x + PLX[4 * dir + 1] + (y + PLY[4 * dir + 1]) * cols]

            ctx.lineTo(base.x + IX[dir] * interp, base2.y + IX[dir + 4] * interp)
            paint = true

            x += IX[dir + 4]
            y += IX[dir + 8]
            pdir = dir
        }

        if (paint) {
            ctx.fill()
            ctx.closePath()
            ctx.beginPath()
            paint = false
        }
    }
}

// ---------------------------------------------------------------------------
// Effect export
// ---------------------------------------------------------------------------

export default canvas.stateful(
    'Lava Lamp',
    {
        bCount: num('Blob Count', [1, 18], 7, { group: 'Scene' }),
        bgColor: color('Background', '#0b0312', { group: 'Scene' }),
        bgCycle: toggle('BG Cycle', false, { group: 'Scene' }),
        color1: color('Color 1', '#16d1d9', { group: 'Color' }),
        color2: color('Color 2', '#ff4fb4', { group: 'Color' }),
        color3: color('Color 3', '#7d49ff', { group: 'Color' }),
        cycleSpeed: num('Cycle Speed', [1, 100], 22, { group: 'Motion' }),
        rainbow: toggle('Rainbow', false, { group: 'Color' }),
        speed: num('Speed', [1, 100], 22, { group: 'Motion' }),
        theme: combo('Theme', THEMES, { group: 'Color' }),
    },
    () => {
        let balls: Ball[] = []
        let grid: GridPoint[] = []
        let sx = 0
        let sy = 0
        let prevWidth = 0
        let prevHeight = 0
        let prevCount = 0
        const msState = { iter: 0, sign: 1 }
        let bgCycleHue = 0
        let rainbowHue = 0

        return (ctx, _time, c) => {
            const bgColor = c.bgColor as string
            const bgCycle = c.bgCycle as boolean
            const theme = c.theme as string
            const color1 = c.color1 as string
            const color2 = c.color2 as string
            const color3 = c.color3 as string
            const rainbow = c.rainbow as boolean
            const speed = c.speed as number
            const cycleSpeed = c.cycleSpeed as number
            const bCount = Math.round(c.bCount as number)

            const w = ctx.canvas.width
            const h = ctx.canvas.height

            // Rebuild on resize or blob count change
            if (w !== prevWidth || h !== prevHeight || bCount !== prevCount) {
                balls = createBalls(bCount, w, h)
                sx = Math.floor(w / STEP)
                sy = Math.floor(h / STEP)
                grid = createGrid(sx, sy)
                msState.iter = 0
                msState.sign = 1
                prevWidth = w
                prevHeight = h
                prevCount = bCount
            }

            // Background fill (with optional hue cycling)
            if (bgCycle) {
                bgCycleHue = (bgCycleHue + cycleSpeed / 50) % 360
                const [, s, l] = rgbToHsl(hexToRgb(bgColor))
                ctx.fillStyle = hslCss(bgCycleHue, s, l)
            } else {
                ctx.fillStyle = bgColor
            }
            ctx.fillRect(0, 0, w, h)

            // Subtle ambient glow from color3
            const palette = resolvePalette(theme, color1, color2, color3)
            const glowRgb = hexToRgb(palette.color3)
            const glow = ctx.createRadialGradient(w * 0.5, h * 0.65, 0, w * 0.5, h * 0.65, w * 0.55)
            glow.addColorStop(0, `rgba(${glowRgb.r},${glowRgb.g},${glowRgb.b},0.07)`)
            glow.addColorStop(1, 'rgba(0,0,0,0)')
            ctx.fillStyle = glow
            ctx.fillRect(0, 0, w, h)

            // Resolve lava fill colors
            let c0: string
            let c1: string

            if (rainbow) {
                rainbowHue = (rainbowHue + cycleSpeed / 50) % 360
                c0 = hslCss(rainbowHue, 1, 0.5)
                c1 = hslCss(rainbowHue + 60, 1, 0.5)
            } else {
                c0 = palette.color1
                c1 = palette.color2
            }

            // Radial gradient fill — centered at bottom-right for diagonal color sweep
            const gradient = ctx.createRadialGradient(w, h, 0, w, h, w)
            gradient.addColorStop(0, c0)
            gradient.addColorStop(1, c1)

            // Animate and render
            moveBalls(balls, speed, w, h)
            renderMetaballs(ctx, grid, sx, sy, balls, gradient, msState)
        }
    },
    {
        description:
            'Molten blobs rise and merge in slow convection. Smooth organic contours glow with radiant gradient heat.',
        presets: [
            {
                controls: {
                    bCount: 5,
                    bgColor: '#1a0800',
                    bgCycle: false,
                    color1: '#ff4400',
                    color2: '#ff8c00',
                    color3: '#cc2200',
                    cycleSpeed: 15,
                    rainbow: false,
                    speed: 18,
                    theme: 'Molten',
                },
                description:
                    "Molten silicate blobs churn in primordial magma. The young Earth's crust fractures and remelts in slow geological fury.",
                name: 'Hadean Mantle Convection',
            },
            {
                controls: {
                    bCount: 12,
                    bgColor: '#020818',
                    bgCycle: false,
                    color1: '#00ffd5',
                    color2: '#4488ff',
                    color3: '#0022aa',
                    cycleSpeed: 30,
                    rainbow: false,
                    speed: 12,
                    theme: 'Lagoon',
                },
                description:
                    'Translucent medusae drift upward through midnight water. Their bells pulse with stolen bioluminescence.',
                name: 'Abyssal Jellyfish Bloom',
            },
            {
                controls: {
                    bCount: 8,
                    bgColor: '#050a02',
                    bgCycle: false,
                    color1: '#36ff9a',
                    color2: '#0ae0cb',
                    color3: '#6c2bff',
                    cycleSpeed: 40,
                    rainbow: false,
                    speed: 28,
                    theme: 'Toxic',
                },
                description:
                    'Alien cytoplasm divides in toxic green mitosis. Each blob a living organelle in some vast extraterrestrial cell.',
                name: 'Xenobiological Specimen',
            },
            {
                controls: {
                    bCount: 6,
                    bgColor: '#0a0a12',
                    bgCycle: true,
                    color1: '#ff4ed6',
                    color2: '#8f48ff',
                    color3: '#42d9ff',
                    cycleSpeed: 65,
                    rainbow: true,
                    speed: 35,
                    theme: 'Synthwave',
                },
                description:
                    'Liquid metal spheres collide and merge in freefall. Rainbow-sheened quicksilver dances in the vacuum.',
                name: 'Mercury in Zero Gravity',
            },
            {
                controls: {
                    bCount: 18,
                    bgColor: '#12041a',
                    bgCycle: false,
                    color1: '#ff4f9a',
                    color2: '#ff74c5',
                    color3: '#8a5cff',
                    cycleSpeed: 20,
                    rainbow: false,
                    speed: 8,
                    theme: 'Bubblegum',
                },
                description:
                    'Warm bubblegum globules rise through a candy-colored incubator. Gentle, hypnotic, impossibly soft.',
                name: 'Thermal Nursery',
            },
            {
                controls: {
                    bCount: 3,
                    bgColor: '#000000',
                    bgCycle: false,
                    color1: '#33f587',
                    color2: '#3fdcff',
                    color3: '#8c4bff',
                    cycleSpeed: 10,
                    rainbow: false,
                    speed: 6,
                    theme: 'Aurora',
                },
                description:
                    'Three ancient glacial masses calve and collide in polar darkness. Jade and sapphire light trapped inside the ice.',
                name: 'Glacial Convergence',
            },
            {
                controls: {
                    bCount: 14,
                    bgColor: '#0f0305',
                    bgCycle: true,
                    color1: '#ffb347',
                    color2: '#ff7a2f',
                    color3: '#ff5778',
                    cycleSpeed: 80,
                    rainbow: false,
                    speed: 45,
                    theme: 'Citrus',
                },
                description:
                    'A solar flare erupts across the chromosphere. Tangerine plasma arcs and collapses in magnetic frenzy.',
                name: 'Chromosphere Eruption',
            },
        ],
    },
)
