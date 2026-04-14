import { canvas, color, combo, num, scaleContext, toggle } from '@hypercolor/sdk'

import { BUILTIN_DESIGN_BASIS, hexToRgb, rgbToCss, scaleRgb, withLift } from '../_builtin/common'

const ORIENTATION_COLORS = ['#80ffea', '#ff6ac1', '#ff6363', '#50fa7b'].map(hexToRgb)

function fract(value: number): number {
    return value - Math.floor(value)
}

function directionAngle(direction: string): number {
    if (direction === 'Top to Bottom') return Math.PI * 0.5
    if (direction === 'Bottom to Top') return -Math.PI * 0.5
    if (direction === 'Right to Left') return Math.PI
    if (direction === 'Top Left to Bottom Right') return Math.PI * 0.25
    if (direction === 'Bottom Right to Top Left') return -Math.PI * 0.75
    if (direction === 'Top Right to Bottom Left') return Math.PI * 0.75
    if (direction === 'Bottom Left to Top Right') return -Math.PI * 0.25
    return 0
}

function sequenceForDirection(direction: string): number[] {
    if (direction === 'Counter Clockwise') return [0, 3, 2, 1]
    if (direction === 'Right to Left') return [1, 0, 3, 2]
    if (direction === 'Bottom to Top') return [3, 2, 1, 0]
    return [0, 1, 2, 3]
}

function orbitPoint(direction: string, phase: number, width: number, height: number): [number, number] {
    const cx = width / 2
    const cy = height / 2

    if (direction === 'Clockwise' || direction === 'Counter Clockwise') {
        const sign = direction === 'Counter Clockwise' ? -1 : 1
        const angle = phase * Math.PI * 2 * sign - Math.PI / 2
        return [cx + Math.cos(angle) * width * 0.32, cy + Math.sin(angle) * height * 0.3]
    }

    if (direction === 'Outward' || direction === 'Inward') return [cx, cy]

    const t = direction === 'Inward' ? 1 - phase : phase
    if (direction === 'Left to Right') return [t * width, cy]
    if (direction === 'Right to Left') return [(1 - t) * width, cy]
    if (direction === 'Top to Bottom') return [cx, t * height]
    if (direction === 'Bottom to Top') return [cx, (1 - t) * height]
    if (direction === 'Top Left to Bottom Right') return [t * width, t * height]
    if (direction === 'Bottom Right to Top Left') return [(1 - t) * width, (1 - t) * height]
    if (direction === 'Top Right to Bottom Left') return [(1 - t) * width, t * height]
    if (direction === 'Bottom Left to Top Right') return [t * width, (1 - t) * height]
    return [t * width, cy]
}

function drawGrid(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    gridScale: number,
    color: string,
): void {
    const columns = Math.max(2, Math.round(gridScale))
    const rows = Math.max(2, Math.round(gridScale * (height / width)))
    ctx.save()
    ctx.strokeStyle = color
    ctx.lineWidth = 1
    ctx.setLineDash([4, 4])

    for (let col = 1; col < columns; col++) {
        const x = (col / columns) * width
        ctx.beginPath()
        ctx.moveTo(x, 0)
        ctx.lineTo(x, height)
        ctx.stroke()
    }

    for (let row = 1; row < rows; row++) {
        const y = (row / rows) * height
        ctx.beginPath()
        ctx.moveTo(0, y)
        ctx.lineTo(width, y)
        ctx.stroke()
    }

    ctx.restore()
}

function drawCornerMarkers(ctx: CanvasRenderingContext2D, width: number, height: number, size: number): void {
    const positions: Array<[number, number]> = [
        [size * 0.7, size * 0.7],
        [width - size * 0.7, size * 0.7],
        [width - size * 0.7, height - size * 0.7],
        [size * 0.7, height - size * 0.7],
    ]

    positions.forEach(([x, y], index) => {
        ctx.save()
        ctx.fillStyle = rgbToCss(ORIENTATION_COLORS[index], 0.92)
        ctx.shadowBlur = size * 0.6
        ctx.shadowColor = rgbToCss(ORIENTATION_COLORS[index], 1)
        ctx.beginPath()
        ctx.arc(x, y, size * 0.28, 0, Math.PI * 2)
        ctx.fill()
        ctx.restore()
    })
}

function drawLinearSweep(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    phase: number,
    direction: string,
    bandWidth: number,
    primary: string,
    secondary: string,
): void {
    const span = Math.hypot(width, height)
    const angle = directionAngle(direction)
    const projection = Math.abs(width * Math.cos(angle)) + Math.abs(height * Math.sin(angle))
    const halfProjection = projection * 0.5
    const position = phase * projection - halfProjection

    ctx.save()
    ctx.translate(width / 2, height / 2)
    ctx.rotate(angle)

    for (const offset of [-projection, 0, projection]) {
        const bandCenter = position + offset
        const gradient = ctx.createLinearGradient(bandCenter - bandWidth, 0, bandCenter + bandWidth, 0)
        gradient.addColorStop(0, 'rgba(255, 255, 255, 0)')
        gradient.addColorStop(0.3, secondary)
        gradient.addColorStop(0.5, primary)
        gradient.addColorStop(0.7, secondary)
        gradient.addColorStop(1, 'rgba(255, 255, 255, 0)')
        ctx.fillStyle = gradient
        ctx.fillRect(bandCenter - bandWidth, -span, bandWidth * 2, span * 2)
    }

    ctx.restore()
}

function drawRingSweep(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    phase: number,
    inward: boolean,
    bandWidth: number,
    primary: string,
    secondary: string,
): void {
    const maxRadius = Math.min(width, height) * 0.48
    const radius = inward ? maxRadius * (1 - phase) : maxRadius * phase
    ctx.save()
    ctx.lineWidth = bandWidth
    ctx.shadowBlur = bandWidth
    ctx.shadowColor = primary
    ctx.strokeStyle = primary
    ctx.beginPath()
    ctx.arc(width / 2, height / 2, radius, 0, Math.PI * 2)
    ctx.stroke()
    ctx.lineWidth = bandWidth * 0.35
    ctx.strokeStyle = secondary
    ctx.beginPath()
    ctx.arc(width / 2, height / 2, radius + bandWidth * 0.3, 0, Math.PI * 2)
    ctx.stroke()
    ctx.restore()
}

export default canvas(
    'Calibration',
    {
        pattern: combo(
            'Pattern',
            ['Sweep', 'Opposing Sweeps', 'Crosshair', 'Quadrant Cycle', 'Corner Cycle', 'Rings'],
            {
                default: 'Sweep',
                group: 'Pattern',
            },
        ),
        direction: combo(
            'Direction',
            [
                'Left to Right',
                'Right to Left',
                'Top to Bottom',
                'Bottom to Top',
                'Top Left to Bottom Right',
                'Bottom Right to Top Left',
                'Top Right to Bottom Left',
                'Bottom Left to Top Right',
                'Clockwise',
                'Counter Clockwise',
                'Outward',
                'Inward',
            ],
            { default: 'Left to Right', group: 'Motion' },
        ),
        speed: num('Sweep Speed', [0, 100], 18, { group: 'Motion' }),
        size: num('Marker Size', [1, 100], 22, { group: 'Motion' }),
        softness: num('Edge Softness', [0, 100], 18, { group: 'Motion' }),
        primary_color: color('Lead Color', '#80ffea', { group: 'Colors' }),
        secondary_color: color('Trail Color', '#ff6ac1', { group: 'Colors' }),
        accent_color: color('Accent Color', '#f8fbff', { group: 'Colors' }),
        background_color: color('Background Color', '#070714', { group: 'Colors' }),
        show_grid: toggle('Show Grid Overlay', false, { group: 'Layout' }),
        grid_scale: num('Grid Scale', [2, 16], 8, { group: 'Layout' }),
        brightness: num('Brightness', [0, 1], 1, { group: 'Output' }),
    },
    (ctx, time, controls) => {
        const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
        const width = s.width
        const height = s.height
        const brightness = controls.brightness as number
        const primary = rgbToCss(scaleRgb(hexToRgb(controls.primary_color as string), brightness))
        const secondary = rgbToCss(scaleRgb(hexToRgb(controls.secondary_color as string), brightness))
        const accent = rgbToCss(scaleRgb(hexToRgb(controls.accent_color as string), brightness))
        const background = rgbToCss(scaleRgb(hexToRgb(controls.background_color as string), brightness))
        const phase = fract(time * (0.04 + ((controls.speed as number) / 100) * 0.24))
        const bandWidth = s.ds(6 + (controls.size as number) * 0.65)
        const softness = (controls.softness as number) / 100
        const pattern = controls.pattern as string
        const direction = controls.direction as string

        ctx.fillStyle = background
        ctx.fillRect(0, 0, width, height)

        if (pattern === 'Sweep' || pattern === 'Opposing Sweeps') {
            if (direction === 'Outward' || direction === 'Inward') {
                drawRingSweep(ctx, width, height, phase, direction === 'Inward', bandWidth, primary, secondary)
                if (pattern === 'Opposing Sweeps') {
                    drawRingSweep(
                        ctx,
                        width,
                        height,
                        fract(phase + 0.5),
                        direction === 'Inward',
                        bandWidth,
                        secondary,
                        primary,
                    )
                }
            } else {
                drawLinearSweep(ctx, width, height, phase, direction, bandWidth, primary, secondary)
                if (pattern === 'Opposing Sweeps') {
                    drawLinearSweep(ctx, width, height, fract(phase + 0.5), direction, bandWidth, secondary, primary)
                }
            }
        } else if (pattern === 'Crosshair') {
            const [x, y] = orbitPoint(direction, phase, width, height)
            ctx.save()
            ctx.strokeStyle = primary
            ctx.lineWidth = Math.max(2, bandWidth * 0.16)
            ctx.shadowBlur = bandWidth * 0.4
            ctx.shadowColor = secondary

            const horizontal = ctx.createLinearGradient(0, y, width, y)
            horizontal.addColorStop(0, 'rgba(255, 255, 255, 0)')
            horizontal.addColorStop(0.5, primary)
            horizontal.addColorStop(1, 'rgba(255, 255, 255, 0)')
            ctx.strokeStyle = horizontal
            ctx.beginPath()
            ctx.moveTo(0, y)
            ctx.lineTo(width, y)
            ctx.stroke()

            const vertical = ctx.createLinearGradient(x, 0, x, height)
            vertical.addColorStop(0, 'rgba(255, 255, 255, 0)')
            vertical.addColorStop(0.5, secondary)
            vertical.addColorStop(1, 'rgba(255, 255, 255, 0)')
            ctx.strokeStyle = vertical
            ctx.beginPath()
            ctx.moveTo(x, 0)
            ctx.lineTo(x, height)
            ctx.stroke()

            ctx.fillStyle = accent
            ctx.beginPath()
            ctx.arc(x, y, bandWidth * 0.16, 0, Math.PI * 2)
            ctx.fill()
            ctx.restore()
        } else if (pattern === 'Quadrant Cycle') {
            const sequence = sequenceForDirection(direction)
            const active = sequence[Math.floor(phase * 4) % 4]
            const quadrants = [
                { color: ORIENTATION_COLORS[0], x: 0, y: 0 },
                { color: ORIENTATION_COLORS[1], x: width / 2, y: 0 },
                { color: ORIENTATION_COLORS[2], x: width / 2, y: height / 2 },
                { color: ORIENTATION_COLORS[3], x: 0, y: height / 2 },
            ]

            quadrants.forEach((quadrant, index) => {
                const emphasis = index === active ? 0.92 : 0.36
                ctx.fillStyle = rgbToCss(scaleRgb(quadrant.color, brightness), emphasis)
                ctx.fillRect(quadrant.x, quadrant.y, width / 2, height / 2)
            })
        } else if (pattern === 'Corner Cycle') {
            const sequence = sequenceForDirection(direction)
            const active = sequence[Math.floor(phase * 4) % 4]
            const points: Array<[number, number]> = [
                [width * 0.14, height * 0.14],
                [width * 0.86, height * 0.14],
                [width * 0.86, height * 0.86],
                [width * 0.14, height * 0.86],
            ]

            points.forEach(([x, y], index) => {
                const colorValue = rgbToCss(scaleRgb(ORIENTATION_COLORS[index], brightness))
                const radius = index === active ? bandWidth * 0.42 : bandWidth * 0.28
                ctx.save()
                ctx.globalAlpha = index === active ? 0.95 : 0.34
                ctx.shadowBlur = bandWidth * 0.55
                ctx.shadowColor = colorValue
                ctx.fillStyle = colorValue
                ctx.beginPath()
                ctx.arc(x, y, radius, 0, Math.PI * 2)
                ctx.fill()
                ctx.restore()
            })
        } else if (pattern === 'Rings') {
            const ringCount = 5
            for (let i = 0; i < ringCount; i++) {
                drawRingSweep(
                    ctx,
                    width,
                    height,
                    fract(phase + i / ringCount),
                    direction === 'Inward',
                    bandWidth * (0.46 + softness * 0.24),
                    i % 2 === 0 ? primary : secondary,
                    accent,
                )
            }
        }

        const centerGuides = ctx.createLinearGradient(0, height / 2, width, height / 2)
        centerGuides.addColorStop(0, 'rgba(255, 255, 255, 0)')
        centerGuides.addColorStop(0.5, rgbToCss(withLift(hexToRgb(controls.accent_color as string), 0.15), 0.18))
        centerGuides.addColorStop(1, 'rgba(255, 255, 255, 0)')
        ctx.fillStyle = centerGuides
        ctx.fillRect(0, height / 2 - 1, width, 2)
        ctx.fillRect(width / 2 - 1, 0, 2, height)

        if (controls.show_grid as boolean) {
            drawGrid(
                ctx,
                width,
                height,
                controls.grid_scale as number,
                rgbToCss(hexToRgb(controls.accent_color as string), 0.28),
            )
        }

        drawCornerMarkers(ctx, width, height, bandWidth)
    },
    {
        author: 'Hypercolor',
        builtinId: 'calibration',
        category: 'utility',
        description:
            'Diagnostic sweeps, quadrants, rings, and corner markers for layout placement, rotation checks, and coverage debugging.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    direction: 'Left to Right',
                    pattern: 'Sweep',
                    show_grid: false,
                    size: 20,
                    softness: 12,
                    speed: 18,
                },
                description: 'Slow left-to-right pass for rough device placement and strip direction checks.',
                name: 'Horizontal Sweep',
            },
            {
                controls: {
                    direction: 'Top to Bottom',
                    pattern: 'Sweep',
                    show_grid: false,
                    size: 20,
                    softness: 12,
                    speed: 18,
                },
                description: 'Top-to-bottom pass for stacked layouts, towers, and vertical strips.',
                name: 'Vertical Sweep',
            },
            {
                controls: {
                    direction: 'Left to Right',
                    grid_scale: 8,
                    pattern: 'Opposing Sweeps',
                    show_grid: true,
                    size: 16,
                    softness: 10,
                    speed: 16,
                },
                description: 'Two mirrored sweeps that make center alignment and mirrored mistakes obvious.',
                name: 'Opposing Edge Scan',
            },
            {
                controls: {
                    direction: 'Top Left to Bottom Right',
                    grid_scale: 10,
                    pattern: 'Crosshair',
                    show_grid: true,
                    size: 14,
                    softness: 16,
                    speed: 22,
                },
                description: 'Moving vertical and horizontal bars whose intersection walks the layout diagonally.',
                name: 'Diagonal Crosshair',
            },
            {
                controls: {
                    direction: 'Clockwise',
                    pattern: 'Quadrant Cycle',
                    size: 24,
                    softness: 8,
                    speed: 20,
                },
                description: 'Clockwise quadrant cycling to verify global orientation at a glance.',
                name: 'Quadrant Clock',
            },
            {
                controls: {
                    direction: 'Clockwise',
                    pattern: 'Corner Cycle',
                    size: 34,
                    softness: 0,
                    speed: 20,
                },
                description: 'Corner beacons cycle around the canvas to expose rotation and mirrored placements.',
                name: 'Corner Compass',
            },
            {
                controls: {
                    direction: 'Outward',
                    grid_scale: 8,
                    pattern: 'Rings',
                    show_grid: true,
                    size: 44,
                    softness: 20,
                    speed: 16,
                },
                description: 'Concentric rings from the center for scale, centering, and radial coverage checks.',
                name: 'Expanding Rings',
            },
            {
                controls: {
                    direction: 'Inward',
                    grid_scale: 8,
                    pattern: 'Rings',
                    show_grid: true,
                    size: 44,
                    softness: 20,
                    speed: 16,
                },
                description: 'Reverse ring motion to confirm center-in vs center-out assumptions.',
                name: 'Inbound Rings',
            },
        ],
    },
)
