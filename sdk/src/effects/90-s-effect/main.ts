import { canvas, color, combo, num } from '@hypercolor/sdk'

interface RGB {
    r: number
    g: number
    b: number
}

interface MotifSeed {
    x: number
    y: number
    size: number
    phase: number
    hueOffset: number
    variant: number
}

const SCENES = ['Pattern 1', 'Pattern 2', 'Pattern 3'] as const
const COLOR_MODES = ['Static', 'Color Cycle'] as const

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function fract(value: number): number {
    return value - Math.floor(value)
}

function hash(value: number): number {
    const s = Math.sin(value * 127.1 + 41.7) * 43758.5453123
    return s - Math.floor(s)
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

function rgbToHsl(color: RGB): { h: number; s: number; l: number } {
    const r = color.r / 255
    const g = color.g / 255
    const b = color.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    const l = (max + min) * 0.5

    if (delta === 0) return { h: 0, s: 0, l }

    const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let h = 0
    if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / delta + 2
    else h = (r - g) / delta + 4

    return { h: h * 60, s, l }
}

function hslToRgb(h: number, s: number, l: number): RGB {
    const hue = ((h % 360) + 360) % 360
    const sat = clamp(s, 0, 1)
    const light = clamp(l, 0, 1)
    const c = (1 - Math.abs(2 * light - 1)) * sat
    const x = c * (1 - Math.abs(((hue / 60) % 2) - 1))
    const m = light - c * 0.5

    let r = 0
    let g = 0
    let b = 0

    if (hue < 60) [r, g, b] = [c, x, 0]
    else if (hue < 120) [r, g, b] = [x, c, 0]
    else if (hue < 180) [r, g, b] = [0, c, x]
    else if (hue < 240) [r, g, b] = [0, x, c]
    else if (hue < 300) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    }
}

function mixRgb(a: RGB, b: RGB, t: number): RGB {
    const ratio = clamp(t, 0, 1)
    return {
        r: Math.round(a.r + (b.r - a.r) * ratio),
        g: Math.round(a.g + (b.g - a.g) * ratio),
        b: Math.round(a.b + (b.b - a.b) * ratio),
    }
}

function enrichRgb(color: RGB, saturationBoost: number, lightnessOffset = 0): RGB {
    const hsl = rgbToHsl(color)
    return hslToRgb(
        hsl.h,
        clamp(hsl.s + saturationBoost, 0, 1),
        clamp(hsl.l + lightnessOffset, 0, 1),
    )
}

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function resolveTone(
    hex: string,
    mode: string,
    time: number,
    cycleSpeed: number,
    hueOffset: number,
    saturationBoost: number,
    lightnessOffset = 0,
): RGB {
    const base = hexToRgb(hex)
    const hsl = rgbToHsl(base)
    const animatedHue = mode === 'Color Cycle'
        ? hsl.h + time * (30 + cycleSpeed * 1.8) + hueOffset
        : hsl.h + hueOffset * 0.08

    return hslToRgb(
        animatedHue,
        clamp(hsl.s + saturationBoost, 0, 1),
        clamp(hsl.l + lightnessOffset, 0, 1),
    )
}

export default canvas.stateful('90\'s Effect', {
    scenes:         combo('Scene', [...SCENES], { default: 'Pattern 1' }),
    frontColor:     color('Front Color', '#00cc93'),
    squiggleColor:  color('Squiggle Color', '#00addb'),
    backColor:      color('Background Color', '#cccc00'),
    colorMode:      combo('Effect Color Mode', [...COLOR_MODES], { default: 'Static' }),
    cycleSpeed:     num('Color Cycle Speed', [0, 100], 50),
    moveSpeed:      num('Animation Speed', [0, 100], 33),
}, () => {
    let lastWidth = 0
    let lastHeight = 0
    let carpetMotifs: MotifSeed[] = []
    let confettiMotifs: MotifSeed[] = []
    let ribbonMotifs: MotifSeed[] = []

    function seedMotifs(width: number, height: number): void {
        if (width === lastWidth && height === lastHeight) return
        lastWidth = width
        lastHeight = height

        carpetMotifs = Array.from({ length: 18 }, (_, index) => ({
            x: hash(index * 1.17 + 0.3),
            y: hash(index * 2.31 + 9.2),
            size: 0.7 + hash(index * 3.93 + 4.2) * 1.1,
            phase: hash(index * 4.71 + 1.8) * Math.PI * 2,
            hueOffset: hash(index * 5.27 + 8.1) * 120 - 60,
            variant: Math.floor(hash(index * 7.11 + 2.4) * 4),
        }))

        confettiMotifs = Array.from({ length: 28 }, (_, index) => ({
            x: hash(index * 1.91 + 7.2),
            y: hash(index * 2.87 + 5.6),
            size: 0.6 + hash(index * 4.13 + 1.2) * 1.5,
            phase: hash(index * 5.73 + 3.4) * Math.PI * 2,
            hueOffset: hash(index * 6.41 + 7.4) * 180 - 90,
            variant: Math.floor(hash(index * 9.1 + 0.7) * 5),
        }))

        ribbonMotifs = Array.from({ length: 4 }, (_, index) => ({
            x: 0,
            y: 0.18 + index * 0.22,
            size: 0.9 + index * 0.26,
            phase: index * 0.9 + hash(index * 7.7) * 0.8,
            hueOffset: index * 42,
            variant: index % 2,
        }))
    }

    function drawBackdrop(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        background: RGB,
        front: RGB,
        squiggle: RGB,
    ): void {
        const top = enrichRgb(background, 0.08, -0.18)
        const bottom = enrichRgb(background, 0.04, -0.05)
        const gradient = ctx.createLinearGradient(0, 0, width, height)
        gradient.addColorStop(0, rgba(top, 1))
        gradient.addColorStop(1, rgba(bottom, 1))
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, width, height)

        const glow = ctx.createRadialGradient(
            width * 0.44,
            height * 0.48,
            0,
            width * 0.44,
            height * 0.48,
            Math.max(width, height) * 0.78,
        )
        glow.addColorStop(0, rgba(mixRgb(front, squiggle, 0.35), 0.14))
        glow.addColorStop(0.55, rgba(squiggle, 0.06))
        glow.addColorStop(1, 'rgba(0, 0, 0, 0)')
        ctx.fillStyle = glow
        ctx.fillRect(0, 0, width, height)

        const grainCount = 48
        for (let i = 0; i < grainCount; i++) {
            const x = hash(i * 1.83 + width * 0.01) * width
            const y = hash(i * 2.39 + height * 0.01) * height
            const alpha = 0.03 + hash(i * 4.11 + 2.9) * 0.04
            ctx.fillStyle = rgba(mixRgb(front, squiggle, i % 2 === 0 ? 0.25 : 0.72), alpha)
            ctx.fillRect(x, y, 1.2, 1.2)
        }
    }

    function drawCarpetStroke(
        ctx: CanvasRenderingContext2D,
        x: number,
        y: number,
        scale: number,
        colorA: RGB,
        colorB: RGB,
    ): void {
        const s = scale
        ctx.lineCap = 'round'
        ctx.lineJoin = 'round'

        ctx.strokeStyle = rgba(colorA, 0.88)
        ctx.lineWidth = Math.max(8, 10 * s)
        ctx.beginPath()
        ctx.moveTo(x, y)
        ctx.lineTo(x + 34 * s, y + 4 * s)
        ctx.lineTo(x + 28 * s, y + 42 * s)
        ctx.lineTo(x + 10 * s, y + 30 * s)
        ctx.lineTo(x + 26 * s, y + 88 * s)
        ctx.stroke()

        ctx.strokeStyle = rgba(colorB, 0.8)
        ctx.lineWidth = Math.max(6, 8 * s)
        ctx.beginPath()
        ctx.moveTo(x + 58 * s, y + 6 * s)
        ctx.lineTo(x + 44 * s, y + 58 * s)
        ctx.lineTo(x + 76 * s, y + 68 * s)
        ctx.lineTo(x + 90 * s, y + 24 * s)
        ctx.lineTo(x + 118 * s, y + 24 * s)
        ctx.lineTo(x + 100 * s, y + 90 * s)
        ctx.stroke()

        ctx.strokeStyle = rgba(colorA, 0.86)
        ctx.lineWidth = Math.max(7, 9 * s)
        ctx.beginPath()
        ctx.moveTo(x + 122 * s, y + 112 * s)
        ctx.lineTo(x + 154 * s, y + 10 * s)
        ctx.lineTo(x + 184 * s, y + 30 * s)
        ctx.lineTo(x + 168 * s, y + 76 * s)
        ctx.lineTo(x + 232 * s, y + 64 * s)
        ctx.lineTo(x + 252 * s, y + 18 * s)
        ctx.stroke()
    }

    function drawPatternOne(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        moveSpeed: number,
        front: RGB,
        squiggle: RGB,
    ): void {
        const drift = time * (14 + moveSpeed * 0.42)
        const rowHeight = height * 0.34
        const colSpacing = width * 0.42

        for (let row = -1; row <= 2; row++) {
            const y = row * rowHeight + fract(drift / Math.max(1, rowHeight)) * rowHeight - rowHeight * 0.22
            for (let col = -1; col <= 2; col++) {
                const x = col * colSpacing - width * 0.08
                drawCarpetStroke(
                    ctx,
                    x,
                    y,
                    0.72 + (row + 1) * 0.04,
                    enrichRgb(squiggle, 0.12, -0.02),
                    enrichRgb(front, 0.08, 0.02),
                )
            }
        }

        ctx.save()
        ctx.globalCompositeOperation = 'screen'
        for (let i = 0; i < carpetMotifs.length; i++) {
            const motif = carpetMotifs[i]
            const laneX = width * (0.14 + (i % 5) * 0.18)
            const travel = fract(motif.y + time * (0.05 + moveSpeed * 0.0007) + motif.phase * 0.03)
            const y = -height * 0.2 + travel * height * 1.45
            const size = (16 + motif.size * 24) * (0.84 + 0.16 * Math.sin(time * 1.4 + motif.phase))
            const color = i % 2 === 0
                ? resolveTone('#ffffff', 'Static', 0, 0, motif.hueOffset, 0, 0)
                : mixRgb(front, squiggle, 0.38)

            ctx.fillStyle = rgba(color, 0.12)
            ctx.beginPath()
            ctx.arc(laneX, y, size * 0.52, 0, Math.PI * 2)
            ctx.fill()
        }
        ctx.restore()

        for (let i = 0; i < carpetMotifs.length; i++) {
            const motif = carpetMotifs[i]
            const lane = i % 3
            const x = width * (0.18 + lane * 0.3) + Math.sin(time * 0.7 + motif.phase) * 12
            const travel = fract(motif.x + time * (0.09 + moveSpeed * 0.0012) + lane * 0.13)
            const y = height * 1.18 - travel * height * 1.6
            const size = 18 + motif.size * 26
            const hueFront = enrichRgb(front, 0.1, 0.02)
            const hueAccent = enrichRgb(squiggle, 0.14, 0.04)

            ctx.save()
            ctx.translate(x, y)
            ctx.rotate(motif.phase * 0.35 + time * 0.4 * (lane === 1 ? -1 : 1))

            if (motif.variant % 3 === 0) {
                ctx.fillStyle = rgba(hueFront, 0.88)
                ctx.beginPath()
                ctx.arc(0, 0, size * 0.44, 0, Math.PI * 2)
                ctx.fill()
                ctx.strokeStyle = rgba(hueAccent, 0.55)
                ctx.lineWidth = Math.max(2, size * 0.08)
                ctx.beginPath()
                ctx.arc(0, 0, size * 0.68, 0, Math.PI * 2)
                ctx.stroke()
            } else if (motif.variant % 3 === 1) {
                ctx.fillStyle = rgba(hueAccent, 0.86)
                ctx.fillRect(-size * 0.22, -size * 0.8, size * 0.44, size * 1.6)
                ctx.fillStyle = rgba(hueFront, 0.46)
                ctx.fillRect(-size * 0.32, -size * 0.5, size * 0.64, size * 0.18)
            } else {
                ctx.fillStyle = rgba(hueFront, 0.88)
                ctx.beginPath()
                ctx.moveTo(0, -size * 0.6)
                ctx.lineTo(-size * 0.54, size * 0.42)
                ctx.lineTo(size * 0.54, size * 0.42)
                ctx.closePath()
                ctx.fill()
                ctx.strokeStyle = rgba(hueAccent, 0.52)
                ctx.lineWidth = Math.max(2, size * 0.08)
                ctx.stroke()
            }

            ctx.restore()
        }
    }

    function drawPatternTwo(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        moveSpeed: number,
        front: RGB,
        squiggle: RGB,
    ): void {
        ctx.save()
        ctx.translate(width * 0.5, height * 0.5)

        const orbit = time * (0.4 + moveSpeed * 0.005)
        const frontHot = enrichRgb(front, 0.18, 0.02)
        const squiggleHot = enrichRgb(squiggle, 0.22, 0.01)

        for (let lane = 0; lane < 4; lane++) {
            const angle = lane % 2 === 0 ? Math.PI * 0.25 : Math.PI * 1.25
            ctx.save()
            ctx.rotate(angle)

            for (let i = -2; i < 6; i++) {
                const offset = fract(i * 0.17 + orbit * 0.22 + lane * 0.13)
                const y = height * 0.7 - offset * height * 1.45
                const color = lane % 2 === 0 ? frontHot : squiggleHot

                ctx.fillStyle = rgba(color, 0.84)
                ctx.fillRect(-14, y, 28, 96)
                ctx.fillStyle = rgba(mixRgb(color, squiggleHot, 0.45), 0.46)
                ctx.fillRect(-22, y + 14, 44, 18)
            }

            ctx.restore()
        }

        for (let lane = 0; lane < confettiMotifs.length; lane++) {
            const motif = confettiMotifs[lane]
            const side = motif.variant % 2 === 0 ? -1 : 1
            const x = side * width * (0.1 + motif.x * 0.34)
            const y = height * 0.78 - fract(motif.y + time * (0.08 + moveSpeed * 0.0015) + motif.phase * 0.04) * height * 1.5
            const size = 12 + motif.size * 22
            const color = motif.variant % 3 === 0
                ? enrichRgb(front, 0.14, 0.04)
                : enrichRgb(squiggle, 0.18, 0.02)

            ctx.save()
            ctx.translate(x, y)
            ctx.rotate(side * 0.78 + Math.sin(time * 1.2 + motif.phase) * 0.18)

            if (motif.variant % 2 === 0) {
                ctx.fillStyle = rgba(color, 0.88)
                ctx.beginPath()
                ctx.moveTo(0, -size * 0.72)
                ctx.lineTo(-size * 0.58, size * 0.42)
                ctx.lineTo(size * 0.58, size * 0.42)
                ctx.closePath()
                ctx.fill()
            } else {
                ctx.fillStyle = rgba(color, 0.86)
                ctx.fillRect(-size * 0.28, -size * 0.82, size * 0.56, size * 1.64)
            }

            ctx.restore()
        }

        ctx.restore()

        for (let i = 0; i < confettiMotifs.length; i++) {
            const motif = confettiMotifs[i]
            const x = motif.x * width
            const y = -18 + fract(motif.y + time * (0.12 + moveSpeed * 0.0017) + i * 0.03) * (height + 36)
            const zig = 8 + motif.size * 6
            ctx.strokeStyle = rgba(enrichRgb(squiggle, 0.18, 0.02), 0.78)
            ctx.lineWidth = 3 + motif.size * 1.4
            ctx.beginPath()
            ctx.moveTo(x, y)
            ctx.lineTo(x + zig * 0.6, y + zig * 0.8)
            ctx.lineTo(x - zig * 0.2, y + zig * 1.4)
            ctx.lineTo(x + zig * 0.8, y + zig * 2.1)
            ctx.stroke()
        }
    }

    function drawRibbonLine(
        ctx: CanvasRenderingContext2D,
        width: number,
        centerY: number,
        thickness: number,
        amplitude: number,
        phase: number,
        colorA: RGB,
        colorB: RGB,
        speed: number,
    ): void {
        const offset = phase + speed * 0.8
        ctx.lineCap = 'round'
        ctx.lineJoin = 'round'

        ctx.strokeStyle = rgba(colorA, 0.86)
        ctx.lineWidth = thickness
        ctx.beginPath()
        for (let x = -20; x <= width + 20; x += 16) {
            const xf = x / width
            const y = centerY
                + Math.sin(xf * Math.PI * 3.2 + offset) * amplitude
                + Math.cos(xf * Math.PI * 6.5 - offset * 1.2) * amplitude * 0.26
            if (x <= -20) ctx.moveTo(x, y)
            else ctx.lineTo(x, y)
        }
        ctx.stroke()

        ctx.strokeStyle = rgba(colorB, 0.58)
        ctx.lineWidth = thickness * 0.38
        ctx.beginPath()
        for (let x = -20; x <= width + 20; x += 16) {
            const xf = x / width
            const y = centerY
                + Math.sin(xf * Math.PI * 3.2 + offset + 0.54) * amplitude * 0.44
                + Math.cos(xf * Math.PI * 5.6 - offset) * amplitude * 0.12
            if (x <= -20) ctx.moveTo(x, y)
            else ctx.lineTo(x, y)
        }
        ctx.stroke()
    }

    function drawPatternThree(
        ctx: CanvasRenderingContext2D,
        width: number,
        height: number,
        time: number,
        moveSpeed: number,
        front: RGB,
        squiggle: RGB,
    ): void {
        const speed = time * (1.2 + moveSpeed * 0.01)
        for (let i = 0; i < ribbonMotifs.length; i++) {
            const ribbon = ribbonMotifs[i]
            const y = height * ribbon.y + Math.sin(speed * 0.6 + ribbon.phase) * 8
            drawRibbonLine(
                ctx,
                width,
                y,
                18 + ribbon.size * 12,
                14 + ribbon.size * 8,
                ribbon.phase,
                ribbon.variant === 0 ? front : squiggle,
                ribbon.variant === 0 ? squiggle : front,
                speed,
            )
        }

        for (let i = 0; i < confettiMotifs.length; i++) {
            const motif = confettiMotifs[i]
            const x = fract(motif.x - time * (0.05 + moveSpeed * 0.0009) - motif.phase * 0.02) * (width + 80) - 40
            const y = height * (0.18 + motif.y * 0.64)
            const size = 6 + motif.size * 9
            const color = i % 2 === 0 ? front : squiggle

            ctx.fillStyle = rgba(color, 0.46)
            ctx.fillRect(x, y - size * 0.5, size * 1.2, size * 0.26)
            ctx.fillRect(x + size * 0.46, y - size, size * 0.26, size * 1.2)
        }
    }

    return (ctx, time, controls) => {
        const width = ctx.canvas.width
        const height = ctx.canvas.height
        seedMotifs(width, height)

        const scene = controls.scenes as string
        const colorMode = controls.colorMode as string
        const cycleSpeed = controls.cycleSpeed as number
        const moveSpeed = controls.moveSpeed as number

        const background = resolveTone(
            controls.backColor as string,
            colorMode,
            time,
            cycleSpeed,
            -80,
            0.08,
            -0.18,
        )
        const front = resolveTone(
            controls.frontColor as string,
            colorMode,
            time,
            cycleSpeed,
            0,
            0.22,
            0.02,
        )
        const squiggle = resolveTone(
            controls.squiggleColor as string,
            colorMode,
            time,
            cycleSpeed,
            100,
            0.26,
            0.01,
        )

        drawBackdrop(ctx, width, height, background, front, squiggle)

        if (scene === 'Pattern 2') {
            drawPatternTwo(ctx, width, height, time, moveSpeed, front, squiggle)
        } else if (scene === 'Pattern 3') {
            drawPatternThree(ctx, width, height, time, moveSpeed, front, squiggle)
        } else {
            drawPatternOne(ctx, width, height, time, moveSpeed, front, squiggle)
        }
    }
}, {
    description: 'Re-live riding a bus in the 90s with bold carpet squiggles and floating retro geometry',
    author: 'Hypercolor',
})
