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

const PARTICLES_PER_DIRECTION = 22
const THEMES = ['Poison', 'Blacklight', 'Radioactive', 'Nightshade', 'Cotton Candy', 'Custom'] as const

const THEME_PALETTES: Record<(typeof THEMES)[number], ThemePalette> = {
    Poison: {
        bg: '#130032',
        colors: ['#6000fc', '#b300ff', '#8a42ff'],
    },
    Blacklight: {
        bg: '#06050d',
        colors: ['#ff58c8', '#30e5ff', '#f4f24e'],
    },
    Radioactive: {
        bg: '#060b05',
        colors: ['#7bff00', '#00ff9d', '#f3ff52'],
    },
    Nightshade: {
        bg: '#0b0615',
        colors: ['#8d5cff', '#ff4fd1', '#56d8ff'],
    },
    'Cotton Candy': {
        bg: '#110816',
        colors: ['#ff74c5', '#79ecff', '#ffe869'],
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
        lineWidth: Math.round(Math.random() * 8) + 2,
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

    function reset(width: number, height: number, paletteSize: number): void {
        particles = []
        for (let i = 0; i < PARTICLES_PER_DIRECTION; i++) {
            particles.push(createParticle(width, height, paletteSize, 1, true))
            particles.push(createParticle(width, height, paletteSize, -1, true))
        }
        lastWidth = width
        lastHeight = height
    }

    function ensureParticleCount(width: number, height: number, paletteSize: number): void {
        const upward = particles.filter((particle) => particle.direction === -1).length
        const downward = particles.filter((particle) => particle.direction === 1).length

        for (let i = downward; i < PARTICLES_PER_DIRECTION; i++) {
            particles.push(createParticle(width, height, paletteSize, 1))
        }
        for (let i = upward; i < PARTICLES_PER_DIRECTION; i++) {
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

        if (width !== lastWidth || height !== lastHeight || particles.length === 0) {
            reset(width, height, palette.length)
        } else {
            ensureParticleCount(width, height, palette.length)
        }

        ctx.fillStyle = speedRaw > 0 ? rgba(background, 0.16) : rgba(background, 1)
        ctx.fillRect(0, 0, width, height)

        for (const particle of particles) {
            const base = palette[particle.colorIndex] ?? palette[0]
            const accent = palette[(particle.colorIndex + 1) % palette.length] ?? mixRgb(base, { r: 255, g: 255, b: 255 }, 0.18)
            const ringStep = Math.max(2.4, particle.radius * 0.18)

            for (let ringIndex = 0; ringIndex < ringCount; ringIndex++) {
                const ringRadius = particle.radius - ringIndex * ringStep
                if (ringRadius <= 1) break

                const blend = ringCount <= 1 ? 0 : ringIndex / Math.max(1, ringCount - 1)
                const ringColor = mixRgb(base, accent, blend * 0.72)
                const alpha = clamp(0.68 - ringIndex * 0.09, 0.24, 0.68)
                const lineWidth = Math.max(1, particle.lineWidth * (1 - ringIndex * 0.16))

                ctx.strokeStyle = rgba(ringColor, alpha)
                ctx.lineWidth = lineWidth
                ctx.beginPath()
                ctx.arc(particle.x, particle.y, ringRadius, 0, Math.PI * 2)
                ctx.stroke()
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
    description: 'A denser concentric-ring poison variant with selectable themes and controllable ring stacks',
    author: 'Hypercolor',
})
