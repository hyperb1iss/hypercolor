import { canvas, combo } from '@hypercolor/sdk'

interface Building {
    x: number
    width: number
    height: number
    layer: number
    windowCols: number
    windowRows: number
    pulse: number
    accent: number
    crown: number
}

interface TransitLane {
    band: number
    offset: number
    speed: number
    length: number
    thickness: number
    direction: 1 | -1
    altitude: number
}

interface Beacon {
    building: number
    offset: number
    pulse: number
    size: number
}

interface PaletteSet {
    bgTop: string
    bgBottom: string
    haze: string
    buildingA: string
    buildingB: string
    windowCool: string
    windowWarm: string
    traffic: string
    beacon: string
    grid: string
}

const COLOR_MODES = ['SilkCircuit', 'Dark Matter', 'Ion Storm', 'Supernova', 'Aurora'] as const
const SCENES = ['Skyline', 'Arcology', 'Rain Grid'] as const

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function hash(value: number): number {
    const seeded = Math.sin(value * 127.1 + 311.7) * 43758.5453123
    return seeded - Math.floor(seeded)
}

function hexToRgb(hex: string): { r: number; g: number; b: number } {
    const normalized = hex.replace('#', '')
    const full = normalized.length === 3
        ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
        : normalized
    const value = Number.parseInt(full, 16)
    return { r: (value >> 16) & 255, g: (value >> 8) & 255, b: value & 255 }
}

function hexToRgba(hex: string, alpha: number): string {
    const color = hexToRgb(hex)
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function getPalette(name: string): PaletteSet {
    if (name === 'SilkCircuit') {
        return {
            bgTop: '#0b0615',
            bgBottom: '#160b27',
            haze: '#e135ff',
            buildingA: '#140d26',
            buildingB: '#22113a',
            windowCool: '#80ffea',
            windowWarm: '#ff6ac1',
            traffic: '#e135ff',
            beacon: '#f1fa8c',
            grid: '#553f88',
        }
    }
    if (name === 'Ion Storm') {
        return {
            bgTop: '#04111e',
            bgBottom: '#0a2033',
            haze: '#58d9ff',
            buildingA: '#08182a',
            buildingB: '#10253b',
            windowCool: '#9be5ff',
            windowWarm: '#62c9ff',
            traffic: '#5ad8ff',
            beacon: '#d8fbff',
            grid: '#284d73',
        }
    }
    if (name === 'Supernova') {
        return {
            bgTop: '#190803',
            bgBottom: '#33140a',
            haze: '#ff7c2a',
            buildingA: '#1d0c06',
            buildingB: '#34120a',
            windowCool: '#ffd3a1',
            windowWarm: '#ff9c4f',
            traffic: '#ff7c2a',
            beacon: '#ffe27a',
            grid: '#6f3421',
        }
    }
    if (name === 'Aurora') {
        return {
            bgTop: '#051412',
            bgBottom: '#0d241f',
            haze: '#43ff95',
            buildingA: '#071a17',
            buildingB: '#122927',
            windowCool: '#cbfff3',
            windowWarm: '#85ffd8',
            traffic: '#43ff95',
            beacon: '#ad7bff',
            grid: '#285349',
        }
    }
    return {
        bgTop: '#050814',
        bgBottom: '#100d21',
        haze: '#8a5bff',
        buildingA: '#090d1f',
        buildingB: '#17132d',
        windowCool: '#b8d1ff',
        windowWarm: '#ff57d6',
        traffic: '#8a5bff',
        beacon: '#82a8ff',
        grid: '#30305d',
    }
}

function computeCounts(density: number): { buildings: number; lanes: number; beacons: number } {
    const normalized = clamp(density, 0, 100) / 100
    return {
        buildings: Math.floor(10 + normalized * 18),
        lanes: Math.floor(4 + normalized * 8),
        beacons: Math.floor(4 + normalized * 10),
    }
}

function sceneBloomAnchors(sceneIndex: number): number[] {
    if (sceneIndex === 1) return [0.18, 0.48, 0.78]
    if (sceneIndex === 2) return [0.14, 0.38, 0.62, 0.86]
    return [0.22, 0.52, 0.82]
}

export default canvas.stateful('Neon City', {
    speed: [1, 10, 4],
    windowDensity: [10, 100, 56],
    trafficFlow: [0, 100, 58],
    haze: [0, 100, 42],
    beacons: true,
    glow: [0, 100, 62],
    colorMode: combo('Palette', [...COLOR_MODES], { default: 'Dark Matter' }),
    scene: combo('Scene', [...SCENES], { default: 'Skyline' }),
}, () => {
    let buildings: Building[] = []
    let lanes: TransitLane[] = []
    let beacons: Beacon[] = []
    let counts = computeCounts(56)
    let lastDensity = 56
    let initialized = false

    function seedCity(): void {
        buildings = Array.from({ length: counts.buildings }, (_, index) => ({
            x: hash(index * 1.73 + 4.2),
            width: 0.05 + hash(index * 2.11 + 7.4) * 0.12,
            height: 0.20 + hash(index * 3.07 + 1.8) * 0.52,
            layer: hash(index * 5.19 + 2.2),
            windowCols: 2 + Math.floor(hash(index * 4.83 + 9.4) * 5),
            windowRows: 4 + Math.floor(hash(index * 6.13 + 0.8) * 11),
            pulse: hash(index * 8.31 + 2.5) * Math.PI * 2,
            accent: hash(index * 9.17 + 8.3),
            crown: hash(index * 10.31 + 5.6),
        })).sort((left, right) => left.layer - right.layer)

        lanes = Array.from({ length: counts.lanes }, (_, index) => ({
            band: hash(index * 2.9 + 1.1),
            offset: hash(index * 4.1 + 8.8),
            speed: 0.35 + hash(index * 5.7 + 3.3) * 1.2,
            length: 0.08 + hash(index * 6.8 + 0.5) * 0.16,
            thickness: 1 + hash(index * 7.3 + 4.2) * 2.4,
            direction: index % 2 === 0 ? 1 : -1,
            altitude: hash(index * 8.4 + 7.9),
        }))

        beacons = Array.from({ length: counts.beacons }, (_, index) => ({
            building: Math.floor(hash(index * 3.4 + 1.9) * Math.max(1, buildings.length)),
            offset: hash(index * 6.6 + 2.7),
            pulse: hash(index * 9.9 + 4.1) * Math.PI * 2,
            size: 1 + hash(index * 12.3 + 6.2) * 2.5,
        }))
    }

    function drawBackground(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        palette: PaletteSet,
        haze: number,
        sceneIndex: number,
        time: number,
    ): void {
        const gradient = ctx.createLinearGradient(0, 0, 0, h)
        gradient.addColorStop(0, palette.bgTop)
        gradient.addColorStop(1, palette.bgBottom)
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, w, h)

        const bloomY = sceneIndex === 2 ? h * 0.55 : h * 0.72
        const anchors = sceneBloomAnchors(sceneIndex)
        for (const [index, anchor] of anchors.entries()) {
            const drift = Math.sin(time * (0.22 + index * 0.04) + anchor * 8) * w * 0.04
            const x = w * anchor + drift
            const radius = Math.max(w, h) * (sceneIndex === 2 ? 0.42 : 0.34) * (1 + index * 0.08)
            const bloom = ctx.createRadialGradient(x, bloomY, 0, x, bloomY, radius)
            bloom.addColorStop(0, hexToRgba(palette.haze, 0.04 + haze * (0.10 + index * 0.03)))
            bloom.addColorStop(1, hexToRgba(palette.haze, 0))
            ctx.fillStyle = bloom
            ctx.fillRect(0, 0, w, h)
        }

        ctx.strokeStyle = hexToRgba(palette.grid, 0.08 + haze * 0.10)
        ctx.lineWidth = 1

        const horizon = sceneIndex === 1 ? h * 0.44 : sceneIndex === 2 ? h * 0.34 : h * 0.62
        for (let i = 0; i < 7; i++) {
            const y = horizon + i * (8 + sceneIndex * 2)
            ctx.beginPath()
            ctx.moveTo(0, y)
            ctx.lineTo(w, y)
            ctx.stroke()
        }

        if (sceneIndex === 2) {
            for (let x = 0; x < w + 24; x += 24) {
                const drift = Math.sin(time * 0.8 + x * 0.02) * 4
                ctx.beginPath()
                ctx.moveTo(x + drift, 0)
                ctx.lineTo(x + drift, h)
                ctx.stroke()
            }
        }

        const pulseY = horizon - 10 + Math.sin(time * 1.3) * 6
        ctx.fillStyle = hexToRgba(palette.haze, 0.05 + haze * 0.10)
        ctx.fillRect(0, pulseY, w, 8)
    }

    function drawSkySweep(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        palette: PaletteSet,
        glow: number,
        sceneIndex: number,
        time: number,
    ): void {
        const sweepStrength = 0.06 + glow * 0.16
        const sweepWidth = w * (sceneIndex === 1 ? 0.18 : 0.13)
        const sweepCount = sceneIndex === 2 ? 4 : 3

        for (let i = 0; i < sweepCount; i++) {
            const cycle = w / sweepCount
            const anchor = ((time * (14 + sceneIndex * 8 + i * 2.4)) + i * cycle * 1.2) % (w + sweepWidth * 2)
            const x = anchor - sweepWidth
            const gradient = ctx.createLinearGradient(x, 0, x + sweepWidth, 0)
            gradient.addColorStop(0, hexToRgba(palette.haze, 0))
            gradient.addColorStop(0.5, hexToRgba(palette.haze, sweepStrength + i * 0.01))
            gradient.addColorStop(1, hexToRgba(palette.haze, 0))
            ctx.fillStyle = gradient
            ctx.fillRect(x, 0, sweepWidth, h * (sceneIndex === 2 ? 0.82 : 0.66))
        }
    }

    function drawBuildings(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        palette: PaletteSet,
        glow: number,
        sceneIndex: number,
        time: number,
    ): Array<{ x: number; y: number; width: number; height: number }> {
        const layouts: Array<{ x: number; y: number; width: number; height: number }> = []
        const ground = sceneIndex === 1 ? h * 0.90 : h * 0.94

        for (const building of buildings) {
            const layerDrift = (building.layer - 0.5) * (sceneIndex === 0 ? 16 : sceneIndex === 1 ? 24 : 9)
            const districtWave = Math.sin(time * (0.10 + building.layer * 0.08) + building.x * 9 + building.pulse)
            const x = (
                building.x * (w + 80)
                - 40
                + districtWave * layerDrift
                + Math.cos(time * 0.18 + building.x * 11) * (4 + building.layer * 8)
            )
            const width = building.width * w
            const height = building.height * h * (sceneIndex === 1 ? 1.08 : sceneIndex === 2 ? 0.88 : 1)
            const y = ground - height

            layouts.push({ x, y, width, height })

            const facade = ctx.createLinearGradient(x, y, x, y + height)
            facade.addColorStop(0, building.accent > 0.48 ? palette.buildingB : palette.buildingA)
            facade.addColorStop(1, palette.buildingA)
            ctx.fillStyle = facade
            ctx.fillRect(x, y, width, height)

            ctx.fillStyle = hexToRgba(palette.grid, 0.12 + glow * 0.08)
            ctx.fillRect(x, y, width, Math.max(2, height * 0.03))

            const crownGlow = 0.08 + glow * 0.22
            if (building.crown > 0.34) {
                const crownHeight = Math.max(2, height * (0.016 + building.crown * 0.035))
                ctx.fillStyle = hexToRgba(building.accent > 0.5 ? palette.windowWarm : palette.windowCool, crownGlow)
                ctx.fillRect(x, y - crownHeight, width, crownHeight)
            }

            if (building.accent > 0.58) {
                const stripX = x + width * (0.14 + building.crown * 0.58)
                const stripHeight = height * (0.18 + 0.16 * (0.5 + 0.5 * Math.sin(time * (1.0 + building.layer) + building.pulse + building.x * 4)))
                ctx.fillStyle = hexToRgba(palette.traffic, 0.10 + glow * 0.18)
                ctx.fillRect(stripX, y + height * 0.10, Math.max(2, width * 0.045), stripHeight)
            }

            const windowPad = 3
            const usableWidth = width - windowPad * 2
            const usableHeight = height - windowPad * 3
            const cellWidth = usableWidth / building.windowCols
            const cellHeight = usableHeight / building.windowRows

            if (cellWidth < 3 || cellHeight < 3) continue

            for (let row = 0; row < building.windowRows; row++) {
                for (let col = 0; col < building.windowCols; col++) {
                    const id = row * 29.3 + col * 11.7 + building.pulse
                    const districtPhase = building.x * 7.5 + building.layer * 3.8
                    const pulseBand = 0.5 + 0.5 * Math.sin(
                        time * (0.8 + building.layer * 0.8)
                        + row * 0.9
                        - col * 0.4
                        + building.pulse
                        + districtPhase,
                    )
                    const scanBand = 0.5 + 0.5 * Math.sin(
                        time * (1.6 + building.layer)
                        - row * 0.7
                        + building.crown * 6
                        + districtPhase,
                    )
                    const flicker = 0.30 + 0.70 * pulseBand
                    const lit = hash(id * 4.9) > 0.44 - pulseBand * 0.18
                    if (!lit) continue

                    const wx = x + windowPad + col * cellWidth + cellWidth * 0.18
                    const wy = y + windowPad * 2 + row * cellHeight + cellHeight * 0.14
                    const ww = cellWidth * 0.56
                    const wh = cellHeight * 0.46
                    const color = hash(id * 7.2) > 0.68 ? palette.windowWarm : palette.windowCool

                    ctx.fillStyle = hexToRgba(color, 0.10 + flicker * (0.34 + glow * 0.18) + scanBand * 0.08)
                    ctx.fillRect(wx, wy, ww, wh)
                }
            }
        }

        return layouts
    }

    function drawTransit(
        ctx: CanvasRenderingContext2D,
        w: number,
        h: number,
        palette: PaletteSet,
        flow: number,
        glow: number,
        sceneIndex: number,
        time: number,
    ): void {
        if (flow <= 0.01) return

        for (const lane of lanes) {
            const bandY = sceneIndex === 2
                ? h * (0.10 + lane.altitude * 0.72)
                : h * (0.56 + lane.altitude * 0.26)
            const segmentLength = lane.length * w
            const spacing = segmentLength * (1.25 + lane.altitude * 0.9)
            const speed = (28 + lane.speed * 58) * flow
            const offset = (time * speed * lane.direction + lane.offset * spacing * 2) % (spacing * 2)

            for (let cursor = -spacing * 2; cursor < w + spacing * 2; cursor += spacing) {
                const position = cursor + offset

                if (sceneIndex === 2) {
                    const top = (position % (h + 50)) - 25
                    ctx.fillStyle = hexToRgba(palette.traffic, 0.10 + glow * 0.12)
                    ctx.fillRect(w * (0.12 + lane.band * 0.76), top, Math.max(2, lane.thickness), segmentLength * 0.75)

                    ctx.fillStyle = hexToRgba(palette.traffic, 0.22 + glow * 0.24)
                    ctx.fillRect(w * (0.12 + lane.band * 0.76), top + segmentLength * 0.54, Math.max(2, lane.thickness), Math.max(5, segmentLength * 0.22))
                    continue
                }

                const tilt = sceneIndex === 1 ? (lane.direction > 0 ? 0.09 : -0.09) : 0
                const y = bandY + tilt * (position - w * 0.5)

                ctx.fillStyle = hexToRgba(palette.traffic, 0.08 + glow * 0.10)
                ctx.fillRect(position - segmentLength * 0.6, y - lane.thickness, segmentLength, lane.thickness * 2.2)

                ctx.fillStyle = hexToRgba(palette.traffic, 0.22 + glow * 0.28)
                ctx.fillRect(position, y - lane.thickness * 0.8, segmentLength * 0.42, lane.thickness * 1.6)

                ctx.fillStyle = hexToRgba(palette.windowCool, 0.12 + glow * 0.16)
                ctx.fillRect(position + segmentLength * 0.12, y - lane.thickness * 0.5, segmentLength * 0.10, lane.thickness)
            }
        }
    }

    function drawBeaconsLayer(
        ctx: CanvasRenderingContext2D,
        layouts: Array<{ x: number; y: number; width: number; height: number }>,
        palette: PaletteSet,
        glow: number,
        time: number,
    ): void {
        for (const beacon of beacons) {
            const building = layouts[beacon.building % Math.max(1, layouts.length)]
            if (!building) continue

            const x = building.x + building.width * (0.18 + beacon.offset * 0.64)
            const y = building.y + 3
            const blink = 0.45 + 0.55 * (0.5 + 0.5 * Math.sin(time * 2.4 + beacon.pulse))
            const size = 1 + beacon.size

            ctx.fillStyle = hexToRgba(palette.beacon, 0.18 + blink * (0.34 + glow * 0.14))
            ctx.fillRect(x - size * 0.5, y - size * 2.4, size, size * 2)

            ctx.fillStyle = hexToRgba(palette.beacon, 0.12 + blink * (0.12 + glow * 0.10))
            ctx.fillRect(x - 0.5, y - 10 - size * 2.2, 1, 10)

            const sweepWidth = 8 + size * 4
            const sweepAlpha = 0.03 + blink * (0.04 + glow * 0.05)
            ctx.fillStyle = hexToRgba(palette.beacon, sweepAlpha)
            ctx.beginPath()
            ctx.moveTo(x, y - 10)
            ctx.lineTo(x - sweepWidth, y - 28 - size * 2)
            ctx.lineTo(x + sweepWidth, y - 28 - size * 2)
            ctx.closePath()
            ctx.fill()
        }
    }

    return (ctx, time, controls) => {
        const speed = controls.speed as number
        const density = controls.windowDensity as number
        const trafficFlow = controls.trafficFlow as number
        const haze = (controls.haze as number) / 100
        const beaconsEnabled = controls.beacons as boolean
        const glow = (controls.glow as number) / 100
        const palette = getPalette(controls.colorMode as string)
        const sceneIndex = SCENES.indexOf(controls.scene as (typeof SCENES)[number])
        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const t = time * (0.45 + speed * 0.22)

        if (!initialized || density !== lastDensity) {
            counts = computeCounts(density)
            seedCity()
            lastDensity = density
            initialized = true
        }

        drawBackground(ctx, w, h, palette, haze, sceneIndex, t)
        drawSkySweep(ctx, w, h, palette, glow, sceneIndex, t)
        const layouts = drawBuildings(ctx, w, h, palette, glow, sceneIndex, t)
        drawTransit(ctx, w, h, palette, trafficFlow / 100, glow, sceneIndex, t)

        if (beaconsEnabled) {
            drawBeaconsLayer(ctx, layouts, palette, glow, t)
        }
    }
}, {
    description: 'Neon skyline silhouettes with lit windows, transit trails, and rooftop beacons',
    author: 'Hypercolor',
})
