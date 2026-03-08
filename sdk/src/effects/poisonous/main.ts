import { canvas, color, combo, num } from '@hypercolor/sdk'

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

const THEMES = ['Poison', 'Blacklight', 'Radioactive', 'Nightshade', 'Cotton Candy', 'Custom'] as const

const THEME_PALETTES: Record<(typeof THEMES)[number], ThemePalette> = {
    Poison: {
        bg: '#130032',
        colors: ['#6000fc', '#b300ff', '#8a42ff'],
    },
    Blacklight: {
        bg: '#06050d',
        colors: ['#ff58c8', '#30e5ff', '#ffb347'],
    },
    Radioactive: {
        bg: '#060b05',
        colors: ['#5cff24', '#00ff9d', '#ff9a3d'],
    },
    Nightshade: {
        bg: '#0b0615',
        colors: ['#8d5cff', '#ff4fd1', '#56d8ff'],
    },
    'Cotton Candy': {
        bg: '#110816',
        colors: ['#ff74c5', '#79ecff', '#ffb347'],
    },
    Custom: {
        bg: '#130032',
        colors: ['#6000fc', '#b300ff', '#8a42ff'],
    },
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function hexToRgb(hex: string): RGB {
    const normalized = hex.trim().replace('#', '')
    const expanded = normalized.length === 3
        ? normalized.split('').map((char) => `${char}${char}`).join('')
        : normalized

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

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function mixRgb(a: RGB, b: RGB, t: number): RGB {
    const ratio = clamp(t, 0, 1)
    return {
        r: Math.round(a.r + (b.r - a.r) * ratio),
        g: Math.round(a.g + (b.g - a.g) * ratio),
        b: Math.round(a.b + (b.b - a.b) * ratio),
    }
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
            colors: [
                controls.color1 as string,
                controls.color2 as string,
                controls.color3 as string,
            ],
        }
    }

    return THEME_PALETTES[theme]
}

function createParticle(
    width: number,
    height: number,
    paletteSize: number,
    direction: 1 | -1,
    initial = false,
): RingParticle {
    const offscreenMargin = 120
    const radius = Math.random() * 6 + 2
    return {
        x: Math.random() * width,
        y: initial
            ? Math.random() * height
            : direction > 0
                ? -Math.random() * offscreenMargin
                : height + Math.random() * offscreenMargin,
        speedX: (Math.random() - 0.5) * (Math.random() * 0.8 + 0.2),
        speedY: Math.random() * 2.4 + 0.7,
        lineWidth: Math.round(Math.random() * 6) + 3,
        radius,
        colorIndex: Math.floor(Math.random() * paletteSize),
        direction,
    }
}

export default canvas.stateful('Poisonous', {
    theme:   combo('Theme', [...THEMES], { default: 'Poison' }),
    bgColor:  color('Background Color', '#130032'),
    color1:   color('Color 1', '#6000fc'),
    color2:   color('Color 2', '#b300ff'),
    color3:   color('Color 3', '#8a42ff'),
    speedRaw: num('Speed', [0, 100], 14),
    ringCount: num('Rings', [1, 6], 2),
}, () => {
    let particles: RingParticle[] = []
    let lastWidth = 0
    let lastHeight = 0

    function particlesPerDirectionForRings(ringCount: number): number {
        const normalized = clamp((ringCount - 1) / 5, 0, 1)
        return Math.round(6 + normalized * 18)
    }

    function reset(width: number, height: number, paletteSize: number, targetPerDirection: number): void {
        particles = []
        for (let i = 0; i < targetPerDirection; i++) {
            particles.push(createParticle(width, height, paletteSize, 1, true))
            particles.push(createParticle(width, height, paletteSize, -1, true))
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
        const upward = particles.filter((particle) => particle.direction === -1).length
        const downward = particles.filter((particle) => particle.direction === 1).length

        for (let i = downward; i < targetPerDirection; i++) {
            particles.push(createParticle(width, height, paletteSize, 1))
        }
        for (let i = upward; i < targetPerDirection; i++) {
            particles.push(createParticle(width, height, paletteSize, -1))
        }
    }

    return (ctx, _time, controls) => {
        const width = ctx.canvas.width
        const height = ctx.canvas.height
        const themePalette = resolveThemePalette(controls as Record<string, unknown>)
        const palette = themePalette.colors.map((color) => hexToRgb(color))
        const background = hexToRgb(themePalette.bg)
        const speedRaw = controls.speedRaw as number
        const ringCount = Math.round(controls.ringCount as number)
        const speedScale = speedRaw / 50
        const targetPerDirection = particlesPerDirectionForRings(ringCount)

        if (width !== lastWidth || height !== lastHeight || particles.length === 0) {
            reset(width, height, palette.length, targetPerDirection)
        } else {
            ensureParticleCount(width, height, palette.length, targetPerDirection)
        }

        ctx.fillStyle = speedRaw > 0 ? rgba(background, 0.16) : rgba(background, 1)
        ctx.fillRect(0, 0, width, height)

        for (const particle of particles) {
            const base = palette[particle.colorIndex] ?? palette[0]
            const accent = palette[(particle.colorIndex + 1) % palette.length] ?? mixRgb(base, { r: 255, g: 255, b: 255 }, 0.18)
            const ringStep = Math.max(3.6, particle.radius * 0.26)
            const ringGap = Math.max(0.8, ringStep * 0.24)

            for (let ringIndex = 0; ringIndex < ringCount; ringIndex++) {
                const ringRadius = particle.radius - ringIndex * ringStep
                if (ringRadius <= 0.75) break

                const blend = ringCount <= 1 ? 0 : ringIndex / Math.max(1, ringCount - 1)
                const ringColor = mixRgb(base, accent, blend * 0.72)
                const targetBandWidth = Math.max(1.5, particle.lineWidth * (1 - ringIndex * 0.18))
                const bandWidth = Math.min(targetBandWidth, Math.max(1.5, ringStep - ringGap))
                const innerRadius = Math.max(0, ringRadius - bandWidth / 2)
                const outerRadius = ringRadius + bandWidth / 2

                ctx.fillStyle = rgba(ringColor, 1)
                fillRingBand(ctx, particle.x, particle.y, innerRadius, outerRadius)
            }

            particle.x += particle.speedX * speedScale
            particle.y += particle.speedY * speedScale * particle.direction
            if (speedRaw > 0) {
                const growth = (Math.random() / 1.4) * speedScale
                particle.radius += growth
            }
        }

        particles = particles.filter((particle) => {
            if (particle.direction > 0) return particle.y <= height + 60
            return particle.y >= -60
        })
    }
}, {
    description: 'Neon toxin rings drifting through a dark haze with theme and palette controls',
    author: 'Hypercolor',
})
