import { canvas, color, combo, num, scaleContext } from '@hypercolor/sdk'

const POISONOUS_DESIGN_BASIS = { height: 200, width: 320 } as const

interface RGB {
    r: number
    g: number
    b: number
}

interface ThemePalette {
    bg: string
    colors: [string, string, string]
}

interface RingParticle {
    x: number
    y: number
    speedX: number
    speedY: number
    lineWidth: number
    radius: number
    colorIndex: number
    direction: 1 | -1
}

const FULL_CIRCLE = Math.PI * 2

const THEMES = ['Blacklight', 'Cotton Candy', 'Custom', 'Nightshade', 'Poison', 'Radioactive'] as const

const THEME_PALETTES: Record<Exclude<(typeof THEMES)[number], 'Custom'>, ThemePalette> = {
    Blacklight: {
        bg: '#06050d',
        colors: ['#ff58c8', '#30e5ff', '#ffb347'],
    },
    'Cotton Candy': {
        bg: '#110816',
        colors: ['#ff74c5', '#79ecff', '#ffb347'],
    },
    Nightshade: {
        bg: '#0b0615',
        colors: ['#8d5cff', '#ff4fd1', '#56d8ff'],
    },
    Poison: {
        bg: '#130032',
        colors: ['#6000fc', '#b300ff', '#8a42ff'],
    },
    Radioactive: {
        bg: '#060b05',
        colors: ['#5cff24', '#00ff9d', '#ff9a3d'],
    },
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function hexToRgb(hex: string): RGB {
    const normalized = hex.trim().replace('#', '')
    const expanded =
        normalized.length === 3
            ? normalized
                  .split('')
                  .map((char) => `${char}${char}`)
                  .join('')
            : normalized

    if (!/^[0-9a-fA-F]{6}$/.test(expanded)) {
        return { b: 255, g: 255, r: 255 }
    }

    const value = Number.parseInt(expanded, 16)
    return {
        b: value & 255,
        g: (value >> 8) & 255,
        r: (value >> 16) & 255,
    }
}

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function mixRgb(a: RGB, b: RGB, t: number): RGB {
    const ratio = clamp(t, 0, 1)
    return {
        b: Math.round(a.b + (b.b - a.b) * ratio),
        g: Math.round(a.g + (b.g - a.g) * ratio),
        r: Math.round(a.r + (b.r - a.r) * ratio),
    }
}

/** sRGB → Oklab (ported from the SDK palette runtime). */
function rgbToOklab(color: RGB): [number, number, number] {
    const linear = (channel: number): number => {
        const c = channel / 255
        return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
    }
    const lr = linear(color.r)
    const lg = linear(color.g)
    const lb = linear(color.b)

    const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
    const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
    const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)

    return [
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ]
}

/** Oklab → sRGB (ported from the SDK palette runtime). */
function oklabToRgb(lightness: number, a: number, b: number): RGB {
    const lRoot = lightness + 0.3963377774 * a + 0.2158037573 * b
    const mRoot = lightness - 0.1055613458 * a - 0.0638541728 * b
    const sRoot = lightness - 0.0894841775 * a - 1.291485548 * b

    const l = lRoot * lRoot * lRoot
    const m = mRoot * mRoot * mRoot
    const s = sRoot * sRoot * sRoot

    const compress = (channel: number): number => {
        const c = channel <= 0.0031308 ? 12.92 * channel : 1.055 * channel ** (1 / 2.4) - 0.055
        return Math.round(clamp(c, 0, 1) * 255)
    }

    return {
        b: compress(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
        g: compress(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        r: compress(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    }
}

/** Perceptual blend between two colors — avoids the muddy midpoints of sRGB mixing. */
function mixOklab(a: RGB, b: RGB, t: number): RGB {
    const ratio = clamp(t, 0, 1)
    const from = rgbToOklab(a)
    const to = rgbToOklab(b)
    return oklabToRgb(
        from[0] + (to[0] - from[0]) * ratio,
        from[1] + (to[1] - from[1]) * ratio,
        from[2] + (to[2] - from[2]) * ratio,
    )
}

function canvasScale(width: number, height: number): number {
    return Math.max(0.5, scaleContext({ height, width }, POISONOUS_DESIGN_BASIS).scale)
}

function fillRingBand(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    innerRadius: number,
    outerRadius: number,
): void {
    ctx.beginPath()
    ctx.arc(x, y, outerRadius, 0, FULL_CIRCLE)
    ctx.arc(x, y, innerRadius, 0, FULL_CIRCLE, true)
    ctx.closePath()
    ctx.fill('evenodd')
}

function resolveThemePalette(controls: Record<string, unknown>): ThemePalette {
    const theme = controls.theme as (typeof THEMES)[number]
    if (theme === 'Custom') {
        return {
            bg: controls.bgColor as string,
            colors: [controls.color1 as string, controls.color2 as string, controls.color3 as string],
        }
    }

    return THEME_PALETTES[theme]
}

function createParticle(
    width: number,
    height: number,
    paletteSize: number,
    direction: 1 | -1,
    scale: number,
    initial = false,
): RingParticle {
    const offscreenMargin = 120 * scale
    const radius = (Math.random() * 6 + 2) * scale
    return {
        colorIndex: Math.floor(Math.random() * paletteSize),
        direction,
        lineWidth: (Math.round(Math.random() * 6) + 3) * scale,
        radius,
        speedX: (Math.random() - 0.5) * (Math.random() * 0.8 + 0.2),
        speedY: Math.random() * 2.4 + 0.7,
        x: Math.random() * width,
        y: initial
            ? Math.random() * height
            : direction > 0
              ? -Math.random() * offscreenMargin
              : height + Math.random() * offscreenMargin,
    }
}

export default canvas.stateful(
    'Poisonous',
    {
        theme: combo('Theme', [...THEMES], { default: 'Poison', group: 'Color' }),
        color1: color('Color 1', '#6000fc', { group: 'Color' }),
        color2: color('Color 2', '#b300ff', { group: 'Color' }),
        color3: color('Color 3', '#8a42ff', { group: 'Color' }),
        bgColor: color('Background Color', '#130032', { group: 'Color' }),
        speedRaw: num('Speed', [0, 100], 14, { group: 'Physics' }),
        ringCount: num('Rings', [1, 6], 2, { group: 'Physics' }),
    },
    () => {
        let particles: RingParticle[] = []
        let lastWidth = 0
        let lastHeight = 0
        let lastTime = -1

        function particlesPerDirectionForRings(ringCount: number): number {
            const normalized = clamp((ringCount - 1) / 5, 0, 1)
            return Math.round(6 + normalized * 18)
        }

        function reset(width: number, height: number, paletteSize: number, targetPerDirection: number): void {
            const scale = canvasScale(width, height)
            particles = []
            for (let i = 0; i < targetPerDirection; i++) {
                particles.push(createParticle(width, height, paletteSize, 1, scale, true))
                particles.push(createParticle(width, height, paletteSize, -1, scale, true))
            }
            lastWidth = width
            lastHeight = height
        }

        function ensureParticleCount(
            width: number,
            height: number,
            paletteSize: number,
            targetPerDirection: number,
        ): void {
            const scale = canvasScale(width, height)
            const upward = particles.filter((particle) => particle.direction === -1).length
            const downward = particles.filter((particle) => particle.direction === 1).length

            for (let i = downward; i < targetPerDirection; i++) {
                particles.push(createParticle(width, height, paletteSize, 1, scale))
            }
            for (let i = upward; i < targetPerDirection; i++) {
                particles.push(createParticle(width, height, paletteSize, -1, scale))
            }
        }

        return (ctx, time, controls) => {
            const width = ctx.canvas.width
            const height = ctx.canvas.height
            const themePalette = resolveThemePalette(controls as Record<string, unknown>)
            const palette = themePalette.colors.map((color) => hexToRgb(color))
            const background = hexToRgb(themePalette.bg)
            const speedRaw = controls.speedRaw as number
            const ringCount = Math.round(controls.ringCount as number)
            const speedScale = speedRaw / 50
            const targetPerDirection = particlesPerDirectionForRings(ringCount)
            // Delta-time keeps motion identical across the daemon's FPS tiers,
            // scaled so per-frame speeds match the original 60fps tuning.
            const dt = lastTime < 0 ? 1 / 60 : Math.min(0.05, Math.max(0, time - lastTime))
            lastTime = time
            const frameScale = dt * 60

            if (width !== lastWidth || height !== lastHeight || particles.length === 0) {
                reset(width, height, palette.length, targetPerDirection)
            } else {
                ensureParticleCount(width, height, palette.length, targetPerDirection)
            }

            // Framerate-compensated trail fade: 0.16/frame at 60fps regardless of tier.
            const fadeAlpha = 1 - 0.84 ** frameScale
            ctx.fillStyle = speedRaw > 0 ? rgba(background, fadeAlpha) : rgba(background, 1)
            ctx.fillRect(0, 0, width, height)

            for (const particle of particles) {
                const base = palette[particle.colorIndex] ?? palette[0]
                const accent =
                    palette[(particle.colorIndex + 1) % palette.length] ??
                    mixRgb(base, { b: 255, g: 255, r: 255 }, 0.18)
                const ringStep = Math.max(3.6, particle.radius * 0.26)
                const ringGap = Math.max(0.8, ringStep * 0.24)

                for (let ringIndex = 0; ringIndex < ringCount; ringIndex++) {
                    const ringRadius = particle.radius - ringIndex * ringStep
                    if (ringRadius <= 0.75) break

                    const blend = ringCount <= 1 ? 0 : ringIndex / Math.max(1, ringCount - 1)
                    const ringColor = mixOklab(base, accent, blend * 0.72)
                    const targetBandWidth = Math.max(1.5, particle.lineWidth * (1 - ringIndex * 0.18))
                    const bandWidth = Math.min(targetBandWidth, Math.max(1.5, ringStep - ringGap))
                    const innerRadius = Math.max(0, ringRadius - bandWidth / 2)
                    const outerRadius = ringRadius + bandWidth / 2

                    ctx.fillStyle = rgba(ringColor, 1)
                    fillRingBand(ctx, particle.x, particle.y, innerRadius, outerRadius)
                }

                particle.x += particle.speedX * speedScale * frameScale
                particle.y += particle.speedY * speedScale * particle.direction * frameScale
                if (speedRaw > 0) {
                    const growth = (Math.random() / 1.4) * speedScale * frameScale
                    particle.radius += growth
                }
            }

            particles = particles.filter((particle) => {
                if (particle.direction > 0) return particle.y <= height + 60
                return particle.y >= -60
            })
        }
    },
    {
        author: 'Hypercolor',
        description:
            'Neon toxin rings pulse through dark chemical haze. Luminous venom drifts in slow vertical procession.',
        designBasis: POISONOUS_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    ringCount: 4,
                    speedRaw: 22,
                    theme: 'Radioactive',
                },
                description:
                    'Bubbling toxic sludge in an abandoned reactor basement. Rings surface through fluorescent green murk.',
                name: 'Radioactive Waste Pool',
            },
            {
                controls: {
                    ringCount: 6,
                    speedRaw: 6,
                    theme: 'Nightshade',
                },
                description: 'Translucent deep-sea bells drift in slow vertical procession through a violet abyss',
                name: 'Jellyfish Ballet',
            },
            {
                controls: {
                    ringCount: 3,
                    speedRaw: 38,
                    theme: 'Cotton Candy',
                },
                description: "Hot pink potions bubble in a candy witch's workshop. Sweet, menacing, impossibly bright.",
                name: 'Bubblegum Cauldron',
            },
            {
                controls: {
                    ringCount: 5,
                    speedRaw: 0,
                    theme: 'Poison',
                },
                description:
                    'Zero-speed suspended rings caught mid-drift. A museum display of crystallized neon toxins.',
                name: 'Frozen in Amber',
            },
            {
                controls: {
                    ringCount: 2,
                    speedRaw: 58,
                    theme: 'Blacklight',
                },
                description:
                    'Blacklight rings expand outward from invisible dancers. Orange, cyan, and magenta halos in the dark.',
                name: 'UV Dance Floor',
            },
            {
                controls: {
                    bgColor: '#020200',
                    color1: '#ff3300',
                    color2: '#ff8800',
                    color3: '#ffb300',
                    ringCount: 1,
                    speedRaw: 90,
                    theme: 'Custom',
                },
                description:
                    'Molten iron droplets launch from a forge and streak upward through the furnace draft. Single rings, maximum velocity.',
                name: 'Blacksmith Sparks',
            },
            {
                controls: {
                    bgColor: '#0a0012',
                    color1: '#cc00ff',
                    color2: '#4400ff',
                    color3: '#0044ff',
                    ringCount: 3,
                    speedRaw: 28,
                    theme: 'Custom',
                },
                description:
                    'Spectral orbs ascend through a cathedral of black glass. Indigo and ultraviolet halos marking each vanished soul.',
                name: 'Ghost Lantern Procession',
            },
        ],
    },
)
