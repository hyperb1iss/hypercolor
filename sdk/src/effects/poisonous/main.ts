import { canvas, color, num } from '@hypercolor/sdk'

interface RGB {
    r: number
    g: number
    b: number
}

interface RingParticle {
    x: number
    y: number
    speedX: number
    speedY: number
    lineWidth: number
    radius: number
    innerRadius: number
    colorIndex: number
    direction: 1 | -1
}

const PARTICLES_PER_DIRECTION = 22

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
        innerRadius: radius * 0.45,
        colorIndex: Math.floor(Math.random() * paletteSize),
        direction,
    }
}

export default canvas.stateful('Poisonous', {
    bgColor:  color('Background Color', '#130032'),
    color1:   color('Color 1', '#6000fc'),
    color2:   color('Color 2', '#b300ff'),
    color3:   color('Color 3', '#8a42ff'),
    speedRaw: num('Speed', [0, 100], 14),
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
        const palette = [
            hexToRgb(controls.color1 as string),
            hexToRgb(controls.color2 as string),
            hexToRgb(controls.color3 as string),
        ]
        const background = hexToRgb(controls.bgColor as string)
        const speedRaw = controls.speedRaw as number
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
            const accent = mixRgb(base, { r: 255, g: 255, b: 255 }, 0.18)

            ctx.strokeStyle = rgba(base, 0.62)
            ctx.lineWidth = particle.lineWidth
            ctx.beginPath()
            ctx.arc(particle.x, particle.y, particle.radius, 0, Math.PI * 2)
            ctx.stroke()

            ctx.strokeStyle = rgba(accent, 0.46)
            ctx.lineWidth = Math.max(1, particle.lineWidth * 0.45)
            ctx.beginPath()
            ctx.arc(particle.x, particle.y, particle.innerRadius, 0, Math.PI * 2)
            ctx.stroke()

            particle.x += particle.speedX * speedScale
            particle.y += particle.speedY * speedScale * particle.direction
            if (speedRaw > 0) {
                const growth = (Math.random() / 1.4) * speedScale
                particle.radius += growth
                particle.innerRadius += growth * 0.72
            }
        }

        particles = particles.filter((particle) => {
            if (particle.direction > 0) return particle.y <= height + 60
            return particle.y >= -60
        })
    }
}, {
    description: 'A denser concentric-ring poison variant with faster motion and cleaner double-circle detail',
    author: 'Hypercolor',
})
