import { canvas, combo, num, toggle } from '@hypercolor/sdk'

type Silhouette = 'flat' | 'stepped' | 'spire' | 'dome'

interface Building {
    x: number
    width: number
    height: number
    layer: number
    windowCols: number
    windowRows: number
    pulse: number
    silhouette: Silhouette
    rimLight: number
    accentStrip: number
    spireHeight: number
    aircraftWarning: boolean
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

const COLOR_MODES = [
    'Akira',
    'Bioluminescent',
    'Blade Runner',
    'Hollow Moon',
    'Ion Chill',
    'Mars Colony',
    'Neo Tokyo',
    'Outrun',
    'Void Matter',
    'Wasabi',
] as const

const SCENES = ['Arcology', 'Rain Grid', 'Skyline'] as const

const PALETTES: Record<(typeof COLOR_MODES)[number], PaletteSet> = {
    Akira: {
        beacon: '#ffd639',
        bgBottom: '#1a0412',
        bgTop: '#08010a',
        buildingA: '#0a0208',
        buildingB: '#210a14',
        grid: '#3a0a18',
        haze: '#ff1a4f',
        traffic: '#ff0a3e',
        windowCool: '#ff4770',
        windowWarm: '#ffb347',
    },
    Bioluminescent: {
        beacon: '#b366ff',
        bgBottom: '#061a2e',
        bgTop: '#020812',
        buildingA: '#031a16',
        buildingB: '#0a2e2a',
        grid: '#1a4a42',
        haze: '#00ffa1',
        traffic: '#00ff95',
        windowCool: '#63ffd6',
        windowWarm: '#c1ff45',
    },
    'Blade Runner': {
        beacon: '#00e5ff',
        bgBottom: '#0e1e2e',
        bgTop: '#050818',
        buildingA: '#050a12',
        buildingB: '#0d1722',
        grid: '#1e3850',
        haze: '#ff8c42',
        traffic: '#ff6b35',
        windowCool: '#ffb870',
        windowWarm: '#ff4d2d',
    },
    'Hollow Moon': {
        beacon: '#ff1a4f',
        bgBottom: '#0a1025',
        bgTop: '#02030a',
        buildingA: '#060814',
        buildingB: '#0c1226',
        grid: '#1e2e5a',
        haze: '#4d74ff',
        traffic: '#ff4d7d',
        windowCool: '#8fb5ff',
        windowWarm: '#b8c7e5',
    },
    'Ion Chill': {
        beacon: '#ffffff',
        bgBottom: '#051529',
        bgTop: '#020716',
        buildingA: '#030a18',
        buildingB: '#0a1e35',
        grid: '#1e4770',
        haze: '#4fc3f7',
        traffic: '#00e5ff',
        windowCool: '#a8e9ff',
        windowWarm: '#ffd866',
    },
    'Mars Colony': {
        beacon: '#00d9ff',
        bgBottom: '#3d0f0a',
        bgTop: '#0f0205',
        buildingA: '#1a0705',
        buildingB: '#3a140c',
        grid: '#602614',
        haze: '#ff6b1a',
        traffic: '#ff8a3d',
        windowCool: '#4fc3f7',
        windowWarm: '#ffb454',
    },
    'Neo Tokyo': {
        beacon: '#f1fa8c',
        bgBottom: '#1a0a2e',
        bgTop: '#0a041a',
        buildingA: '#0e0520',
        buildingB: '#1f0f3a',
        grid: '#4a2d7a',
        haze: '#e135ff',
        traffic: '#ff2d8f',
        windowCool: '#80ffea',
        windowWarm: '#ff6ac1',
    },
    Outrun: {
        beacon: '#60fff7',
        bgBottom: '#2d0b4d',
        bgTop: '#0d0331',
        buildingA: '#0d0428',
        buildingB: '#1f093a',
        grid: '#5c2d8a',
        haze: '#ff2f8e',
        traffic: '#ff91a9',
        windowCool: '#9d4eff',
        windowWarm: '#ff6ac1',
    },
    'Void Matter': {
        beacon: '#ffd166',
        bgBottom: '#100a1f',
        bgTop: '#05030a',
        buildingA: '#060410',
        buildingB: '#160c25',
        grid: '#36295e',
        haze: '#8a5bff',
        traffic: '#7a42ff',
        windowCool: '#b8d1ff',
        windowWarm: '#ff57d6',
    },
    Wasabi: {
        beacon: '#ff6ac1',
        bgBottom: '#1d2604',
        bgTop: '#0a0d02',
        buildingA: '#0d1003',
        buildingB: '#1f2808',
        grid: '#3d4e0a',
        haze: '#d0ff2d',
        traffic: '#a8ff1e',
        windowCool: '#80ffea',
        windowWarm: '#ff2d8f',
    },
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function hash(value: number): number {
    const seeded = Math.sin(value * 127.1 + 311.7) * 43758.5453123
    return seeded - Math.floor(seeded)
}

function hexToRgb(hex: string): { r: number; g: number; b: number } {
    const normalized = hex.replace('#', '')
    const full =
        normalized.length === 3
            ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
            : normalized
    const value = Number.parseInt(full, 16)
    return { b: value & 255, g: (value >> 8) & 255, r: (value >> 16) & 255 }
}

function hexToRgba(hex: string, alpha: number): string {
    const color = hexToRgb(hex)
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function getPalette(name: string): PaletteSet {
    return PALETTES[name as (typeof COLOR_MODES)[number]] ?? PALETTES['Void Matter']
}

function computeCounts(density: number): { buildings: number; lanes: number; beacons: number } {
    const normalized = clamp(density, 0, 100) / 100
    return {
        beacons: Math.floor(4 + normalized * 10),
        buildings: Math.floor(10 + normalized * 18),
        lanes: Math.floor(4 + normalized * 8),
    }
}

function pickSilhouette(value: number): Silhouette {
    if (value < 0.3) return 'flat'
    if (value < 0.62) return 'stepped'
    if (value < 0.88) return 'spire'
    return 'dome'
}

function sceneBloomAnchors(sceneIndex: number): number[] {
    if (sceneIndex === 1) return [0.18, 0.48, 0.78]
    if (sceneIndex === 2) return [0.14, 0.38, 0.62, 0.86]
    return [0.22, 0.52, 0.82]
}

function sceneHorizon(h: number, sceneIndex: number): number {
    if (sceneIndex === 1) return h * 0.44
    if (sceneIndex === 2) return h * 0.34
    return h * 0.62
}

function sceneGround(h: number, sceneIndex: number): number {
    if (sceneIndex === 1) return h * 0.9
    return h * 0.94
}

function sceneMoonCenter(w: number, h: number, sceneIndex: number, time: number): { x: number; y: number; r: number } {
    const drift = Math.sin(time * 0.05) * w * 0.04
    if (sceneIndex === 1) {
        return { r: Math.max(w, h) * 0.16, x: w * 0.74 + drift, y: h * 0.2 }
    }
    if (sceneIndex === 2) {
        return { r: Math.max(w, h) * 0.22, x: w * 0.28 + drift, y: h * 0.14 }
    }
    return { r: Math.max(w, h) * 0.19, x: w * 0.76 + drift, y: h * 0.3 }
}

export default canvas.stateful(
    'Neon City',
    {
        beacons: toggle('Beacons', true, { group: 'Geometry' }),
        colorMode: combo('Palette', [...COLOR_MODES], { default: 'Neo Tokyo', group: 'Scene' }),
        glow: num('Glow', [0, 100], 62, { group: 'Atmosphere' }),
        haze: num('Haze', [0, 100], 48, { group: 'Atmosphere' }),
        scene: combo('Scene', [...SCENES], { default: 'Skyline', group: 'Scene' }),
        speed: num('Speed', [1, 10], 4, { group: 'Motion' }),
        trafficFlow: num('Traffic Flow', [0, 100], 58, { group: 'Motion' }),
        windowDensity: num('Window Density', [10, 100], 56, { group: 'Geometry' }),
    },
    () => {
        let buildings: Building[] = []
        let lanes: TransitLane[] = []
        let beacons: Beacon[] = []
        let counts = computeCounts(56)
        let lastDensity = 56
        let initialized = false

        function seedCity(): void {
            buildings = Array.from({ length: counts.buildings }, (_, index) => {
                const heightVal = 0.2 + hash(index * 3.07 + 1.8) * 0.52
                const silhouetteHash = hash(index * 10.31 + 5.6)
                return {
                    accentStrip: hash(index * 9.17 + 8.3),
                    aircraftWarning: heightVal > 0.56,
                    height: heightVal,
                    layer: hash(index * 5.19 + 2.2),
                    pulse: hash(index * 8.31 + 2.5) * Math.PI * 2,
                    rimLight: hash(index * 11.47 + 3.1),
                    silhouette: pickSilhouette(silhouetteHash),
                    spireHeight: 0.4 + hash(index * 13.7 + 9.2) * 0.8,
                    width: 0.05 + hash(index * 2.11 + 7.4) * 0.12,
                    windowCols: 2 + Math.floor(hash(index * 4.83 + 9.4) * 5),
                    windowRows: 4 + Math.floor(hash(index * 6.13 + 0.8) * 11),
                    x: hash(index * 1.73 + 4.2),
                }
            }).sort((left, right) => left.layer - right.layer)

            lanes = Array.from({ length: counts.lanes }, (_, index) => ({
                altitude: hash(index * 8.4 + 7.9),
                band: hash(index * 2.9 + 1.1),
                direction: index % 2 === 0 ? 1 : -1,
                length: 0.08 + hash(index * 6.8 + 0.5) * 0.16,
                offset: hash(index * 4.1 + 8.8),
                speed: 0.35 + hash(index * 5.7 + 3.3) * 1.2,
                thickness: 1 + hash(index * 7.3 + 4.2) * 2.4,
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
            gradient.addColorStop(0.62, palette.bgBottom)
            gradient.addColorStop(1, palette.buildingA)
            ctx.fillStyle = gradient
            ctx.fillRect(0, 0, w, h)

            const bloomY = sceneIndex === 2 ? h * 0.55 : h * 0.72
            const anchors = sceneBloomAnchors(sceneIndex)
            for (const [index, anchor] of anchors.entries()) {
                const drift = Math.sin(time * (0.22 + index * 0.04) + anchor * 8) * w * 0.04
                const x = w * anchor + drift
                const radius = Math.max(w, h) * (sceneIndex === 2 ? 0.42 : 0.34) * (1 + index * 0.08)
                const bloom = ctx.createRadialGradient(x, bloomY, 0, x, bloomY, radius)
                bloom.addColorStop(0, hexToRgba(palette.haze, 0.04 + haze * (0.1 + index * 0.03)))
                bloom.addColorStop(1, hexToRgba(palette.haze, 0))
                ctx.fillStyle = bloom
                ctx.fillRect(0, 0, w, h)
            }
        }

        function drawMoon(
            ctx: CanvasRenderingContext2D,
            w: number,
            h: number,
            palette: PaletteSet,
            haze: number,
            glow: number,
            sceneIndex: number,
            time: number,
        ): void {
            const moon = sceneMoonCenter(w, h, sceneIndex, time)
            const intensity = 0.28 + glow * 0.32 + haze * 0.16

            const halo = ctx.createRadialGradient(moon.x, moon.y, moon.r * 0.2, moon.x, moon.y, moon.r * 3.4)
            halo.addColorStop(0, hexToRgba(palette.haze, intensity * 0.5))
            halo.addColorStop(0.35, hexToRgba(palette.haze, intensity * 0.18))
            halo.addColorStop(1, hexToRgba(palette.haze, 0))
            ctx.fillStyle = halo
            ctx.fillRect(0, 0, w, h)

            const disk = ctx.createRadialGradient(moon.x, moon.y, 0, moon.x, moon.y, moon.r)
            disk.addColorStop(0, hexToRgba(palette.haze, 0.55 + glow * 0.25))
            disk.addColorStop(0.45, hexToRgba(palette.haze, 0.22 + glow * 0.18))
            disk.addColorStop(1, hexToRgba(palette.haze, 0))
            ctx.fillStyle = disk
            ctx.fillRect(0, 0, w, h)
        }

        function drawHorizonGrid(
            ctx: CanvasRenderingContext2D,
            w: number,
            h: number,
            palette: PaletteSet,
            haze: number,
            sceneIndex: number,
            time: number,
        ): void {
            ctx.strokeStyle = hexToRgba(palette.grid, 0.08 + haze * 0.1)
            ctx.lineWidth = 1

            const horizon = sceneHorizon(h, sceneIndex)
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
            ctx.fillStyle = hexToRgba(palette.haze, 0.05 + haze * 0.1)
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
                const anchor = (time * (14 + sceneIndex * 8 + i * 2.4) + i * cycle * 1.2) % (w + sweepWidth * 2)
                const x = anchor - sweepWidth
                const gradient = ctx.createLinearGradient(x, 0, x + sweepWidth, 0)
                gradient.addColorStop(0, hexToRgba(palette.haze, 0))
                gradient.addColorStop(0.5, hexToRgba(palette.haze, sweepStrength + i * 0.01))
                gradient.addColorStop(1, hexToRgba(palette.haze, 0))
                ctx.fillStyle = gradient
                ctx.fillRect(x, 0, sweepWidth, h * (sceneIndex === 2 ? 0.82 : 0.66))
            }
        }

        function drawBuildingSilhouette(
            ctx: CanvasRenderingContext2D,
            building: Building,
            x: number,
            y: number,
            width: number,
            height: number,
            palette: PaletteSet,
            glow: number,
        ): { spireTipX: number; spireTipY: number } | null {
            const facade = ctx.createLinearGradient(x, y, x, y + height)
            facade.addColorStop(0, building.accentStrip > 0.48 ? palette.buildingB : palette.buildingA)
            facade.addColorStop(1, palette.buildingA)
            ctx.fillStyle = facade
            ctx.fillRect(x, y, width, height)

            ctx.fillStyle = hexToRgba(palette.grid, 0.12 + glow * 0.08)
            ctx.fillRect(x, y, width, Math.max(2, height * 0.03))

            if (building.silhouette === 'stepped') {
                const stepW = width * 0.68
                const stepH = Math.max(4, height * 0.09)
                const stepX = x + (width - stepW) * 0.5
                const stepY = y - stepH
                ctx.fillStyle = palette.buildingB
                ctx.fillRect(stepX, stepY, stepW, stepH)
                ctx.fillStyle = hexToRgba(palette.grid, 0.18 + glow * 0.1)
                ctx.fillRect(stepX, stepY, stepW, Math.max(1, stepH * 0.2))

                const capW = stepW * 0.45
                const capX = stepX + (stepW - capW) * 0.5
                const capH = Math.max(2, stepH * 0.55)
                ctx.fillStyle = palette.buildingA
                ctx.fillRect(capX, stepY - capH, capW, capH)
                return { spireTipX: capX + capW * 0.5, spireTipY: stepY - capH }
            }

            if (building.silhouette === 'spire') {
                const spireW = Math.max(1, width * 0.08)
                const spireX = x + width * 0.5 - spireW * 0.5
                const spireH = height * (0.3 + building.spireHeight * 0.5)
                const spireY = y - spireH
                const spireGrad = ctx.createLinearGradient(spireX, spireY, spireX, y)
                spireGrad.addColorStop(0, palette.buildingA)
                spireGrad.addColorStop(1, palette.buildingB)
                ctx.fillStyle = spireGrad
                ctx.fillRect(spireX, spireY, spireW, spireH)
                return { spireTipX: spireX + spireW * 0.5, spireTipY: spireY }
            }

            if (building.silhouette === 'dome') {
                const domeR = Math.min(width * 0.42, height * 0.14)
                const cx = x + width * 0.5
                const cy = y
                ctx.fillStyle = palette.buildingB
                ctx.beginPath()
                ctx.ellipse(cx, cy, domeR, domeR * 0.72, 0, Math.PI, 0, false)
                ctx.closePath()
                ctx.fill()
                ctx.fillStyle = hexToRgba(palette.grid, 0.14 + glow * 0.08)
                ctx.fillRect(cx - domeR, cy - 1, domeR * 2, 2)
                return { spireTipX: cx, spireTipY: cy - domeR * 0.72 }
            }

            return null
        }

        function drawBuildings(
            ctx: CanvasRenderingContext2D,
            w: number,
            h: number,
            palette: PaletteSet,
            glow: number,
            haze: number,
            sceneIndex: number,
            time: number,
        ): Array<{
            x: number
            y: number
            width: number
            height: number
            layer: number
            spireTipX: number
            spireTipY: number
            aircraftWarning: boolean
            pulse: number
        }> {
            const layouts: Array<{
                x: number
                y: number
                width: number
                height: number
                layer: number
                spireTipX: number
                spireTipY: number
                aircraftWarning: boolean
                pulse: number
            }> = []
            const ground = sceneGround(h, sceneIndex)

            for (const building of buildings) {
                const layerDrift = (building.layer - 0.5) * (sceneIndex === 0 ? 16 : sceneIndex === 1 ? 24 : 9)
                const districtWave = Math.sin(time * (0.1 + building.layer * 0.08) + building.x * 9 + building.pulse)
                const x =
                    building.x * (w + 80) -
                    40 +
                    districtWave * layerDrift +
                    Math.cos(time * 0.18 + building.x * 11) * (4 + building.layer * 8)
                const width = building.width * w
                const height = building.height * h * (sceneIndex === 1 ? 1.08 : sceneIndex === 2 ? 0.88 : 1)
                const y = ground - height

                const silhouetteResult = drawBuildingSilhouette(ctx, building, x, y, width, height, palette, glow)
                const spireTipX = silhouetteResult?.spireTipX ?? x + width * 0.5
                const spireTipY = silhouetteResult?.spireTipY ?? y

                const crownGlow = 0.08 + glow * 0.22
                if (building.rimLight > 0.34 && building.silhouette !== 'dome') {
                    const crownHeight = Math.max(2, height * (0.016 + building.rimLight * 0.035))
                    ctx.fillStyle = hexToRgba(
                        building.accentStrip > 0.5 ? palette.windowWarm : palette.windowCool,
                        crownGlow,
                    )
                    ctx.fillRect(x, y - crownHeight, width, crownHeight)
                }

                if (building.accentStrip > 0.58) {
                    const stripX = x + width * (0.14 + building.rimLight * 0.58)
                    const stripHeight =
                        height *
                        (0.18 +
                            0.16 *
                                (0.5 + 0.5 * Math.sin(time * (1.0 + building.layer) + building.pulse + building.x * 4)))
                    ctx.fillStyle = hexToRgba(palette.traffic, 0.1 + glow * 0.18)
                    ctx.fillRect(stripX, y + height * 0.1, Math.max(2, width * 0.045), stripHeight)
                }

                const windowPad = 3
                const usableWidth = width - windowPad * 2
                const usableHeight = height - windowPad * 3
                const cellWidth = usableWidth / building.windowCols
                const cellHeight = usableHeight / building.windowRows

                if (cellWidth >= 3 && cellHeight >= 3) {
                    for (let row = 0; row < building.windowRows; row++) {
                        for (let col = 0; col < building.windowCols; col++) {
                            const id = row * 29.3 + col * 11.7 + building.pulse
                            const districtPhase = building.x * 7.5 + building.layer * 3.8
                            const pulseBand =
                                0.5 +
                                0.5 *
                                    Math.sin(
                                        time * (0.8 + building.layer * 0.8) +
                                            row * 0.9 -
                                            col * 0.4 +
                                            building.pulse +
                                            districtPhase,
                                    )
                            const scanBand =
                                0.5 +
                                0.5 *
                                    Math.sin(
                                        time * (1.6 + building.layer) -
                                            row * 0.7 +
                                            building.rimLight * 6 +
                                            districtPhase,
                                    )
                            const flicker = 0.3 + 0.7 * pulseBand
                            const lit = hash(id * 4.9) > 0.44 - pulseBand * 0.18
                            if (!lit) continue

                            const wx = x + windowPad + col * cellWidth + cellWidth * 0.18
                            const wy = y + windowPad * 2 + row * cellHeight + cellHeight * 0.14
                            const ww = cellWidth * 0.56
                            const wh = cellHeight * 0.46
                            const color = hash(id * 7.2) > 0.68 ? palette.windowWarm : palette.windowCool

                            ctx.fillStyle = hexToRgba(color, 0.1 + flicker * (0.34 + glow * 0.18) + scanBand * 0.08)
                            ctx.fillRect(wx, wy, ww, wh)
                        }
                    }
                }

                const atmosphereDepth = 1 - building.layer
                const fadeAmount = atmosphereDepth * (0.18 + haze * 0.32)
                if (fadeAmount > 0.02) {
                    ctx.fillStyle = hexToRgba(palette.bgBottom, fadeAmount)
                    const extentTop = Math.min(y, spireTipY) - 2
                    ctx.fillRect(x - 2, extentTop, width + 4, ground - extentTop + 2)
                    if (haze > 0.25) {
                        ctx.fillStyle = hexToRgba(palette.haze, atmosphereDepth * haze * 0.06)
                        ctx.fillRect(x - 2, extentTop, width + 4, ground - extentTop + 2)
                    }
                }

                layouts.push({
                    aircraftWarning: building.aircraftWarning,
                    height,
                    layer: building.layer,
                    pulse: building.pulse,
                    spireTipX,
                    spireTipY,
                    width,
                    x,
                    y,
                })
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
                const bandY = sceneIndex === 2 ? h * (0.42 + lane.altitude * 0.44) : h * (0.56 + lane.altitude * 0.26)
                const segmentLength = lane.length * w
                const spacing = segmentLength * (1.6 + lane.altitude * 1.1)
                const speed = (28 + lane.speed * 58) * flow
                const offset = (time * speed * lane.direction + lane.offset * spacing * 2) % (spacing * 2)
                const dir = lane.direction

                for (let cursor = -spacing * 2; cursor < w + spacing * 2; cursor += spacing) {
                    const position = cursor + offset
                    const tilt = sceneIndex === 1 ? (dir > 0 ? 0.09 : -0.09) : 0
                    const y = bandY + tilt * (position - w * 0.5)

                    const headX = position + segmentLength * 0.35 * dir
                    const tailX = position - segmentLength * 0.7 * dir
                    const leftX = Math.min(headX, tailX)
                    const rightX = Math.max(headX, tailX)
                    const bodyW = rightX - leftX

                    ctx.fillStyle = hexToRgba(palette.traffic, 0.05 + glow * 0.08)
                    ctx.fillRect(leftX - bodyW * 0.1, y - lane.thickness * 1.8, bodyW * 1.2, lane.thickness * 3.6)

                    const bodyGrad = ctx.createLinearGradient(tailX, y, headX, y)
                    bodyGrad.addColorStop(0, hexToRgba(palette.traffic, 0))
                    bodyGrad.addColorStop(0.65, hexToRgba(palette.traffic, 0.28 + glow * 0.28))
                    bodyGrad.addColorStop(1, hexToRgba(palette.windowCool, 0.55 + glow * 0.32))
                    ctx.fillStyle = bodyGrad
                    ctx.fillRect(leftX, y - lane.thickness * 0.9, bodyW, lane.thickness * 1.8)

                    const headPixelX = headX - dir * 2
                    ctx.fillStyle = hexToRgba(palette.windowCool, 0.7 + glow * 0.22)
                    ctx.fillRect(headPixelX - 1.5, y - lane.thickness * 0.5, 3, lane.thickness)

                    ctx.fillStyle = hexToRgba(palette.traffic, 0.35 + glow * 0.2)
                    ctx.fillRect(
                        tailX + dir * segmentLength * 0.05,
                        y - lane.thickness * 0.4,
                        segmentLength * 0.08,
                        lane.thickness * 0.8,
                    )
                }
            }
        }

        function drawWetGround(
            ctx: CanvasRenderingContext2D,
            w: number,
            h: number,
            palette: PaletteSet,
            glow: number,
            haze: number,
            sceneIndex: number,
            time: number,
        ): void {
            const ground = sceneGround(h, sceneIndex)
            const gradient = ctx.createLinearGradient(0, ground, 0, h)
            gradient.addColorStop(0, hexToRgba(palette.traffic, 0.04 + glow * 0.06))
            gradient.addColorStop(0.5, hexToRgba(palette.haze, 0.08 + glow * 0.06 + haze * 0.06))
            gradient.addColorStop(1, hexToRgba(palette.bgBottom, 0.6))
            ctx.fillStyle = gradient
            ctx.fillRect(0, ground, w, h - ground)

            const stripeCount = 5
            const stripeBand = h - ground
            for (let i = 0; i < stripeCount; i++) {
                const stripeT = (i + 0.5) / stripeCount
                const y = ground + stripeT * stripeBand + Math.sin(time * 0.6 + i * 1.7) * 1.5
                const stripeAlpha = 0.05 + (1 - stripeT) * 0.08 * (0.4 + glow * 0.5)
                ctx.fillStyle = hexToRgba(palette.windowCool, stripeAlpha)
                ctx.fillRect(0, y, w, 1)
            }
        }

        function drawRainStreaks(
            ctx: CanvasRenderingContext2D,
            w: number,
            h: number,
            palette: PaletteSet,
            haze: number,
            time: number,
        ): void {
            const count = Math.floor(90 + haze * 60)
            ctx.strokeStyle = hexToRgba(palette.haze, 0.1 + haze * 0.14)
            ctx.lineWidth = 0.9
            ctx.beginPath()
            for (let i = 0; i < count; i++) {
                const seedX = hash(i * 3.7) * w * 1.2 - w * 0.1
                const sway = Math.sin(time * 0.4 + i * 0.13) * 6
                const fall = (hash(i * 7.1) + time * 2.2 + i * 0.007) % 1
                const y = fall * (h * 1.4) - h * 0.2
                const len = 12 + hash(i * 9.3) * 26
                const x = seedX + sway
                ctx.moveTo(x, y)
                ctx.lineTo(x - len * 0.18, y + len)
            }
            ctx.stroke()
        }

        function drawBeaconsLayer(
            ctx: CanvasRenderingContext2D,
            layouts: Array<{
                x: number
                y: number
                width: number
                height: number
                spireTipX: number
                spireTipY: number
                aircraftWarning: boolean
                pulse: number
            }>,
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

                ctx.fillStyle = hexToRgba(palette.beacon, 0.12 + blink * (0.12 + glow * 0.1))
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

        function drawAircraftWarnings(
            ctx: CanvasRenderingContext2D,
            layouts: Array<{ spireTipX: number; spireTipY: number; aircraftWarning: boolean; pulse: number }>,
            palette: PaletteSet,
            glow: number,
            time: number,
        ): void {
            for (const building of layouts) {
                if (!building.aircraftWarning) continue
                const slowBlink = (0.5 + 0.5 * Math.sin(time * 1.1 + building.pulse * 1.7)) ** 3
                const intensity = 0.25 + slowBlink * (0.5 + glow * 0.25)
                ctx.fillStyle = hexToRgba(palette.beacon, intensity)
                ctx.fillRect(building.spireTipX - 1, building.spireTipY - 2, 2, 2)
                ctx.fillStyle = hexToRgba(palette.beacon, intensity * 0.35)
                ctx.fillRect(building.spireTipX - 2.5, building.spireTipY - 3.5, 5, 5)
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
            const isRainGrid = sceneIndex === 1
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
            drawMoon(ctx, w, h, palette, haze, glow, sceneIndex, t)
            drawHorizonGrid(ctx, w, h, palette, haze, sceneIndex, t)
            drawSkySweep(ctx, w, h, palette, glow, sceneIndex, t)

            if (isRainGrid) {
                drawRainStreaks(ctx, w, h, palette, haze, t)
            }

            const layouts = drawBuildings(ctx, w, h, palette, glow, haze, sceneIndex, t)
            drawTransit(ctx, w, h, palette, trafficFlow / 100, glow, sceneIndex, t)

            if (isRainGrid) {
                drawWetGround(ctx, w, h, palette, glow, haze, sceneIndex, t)
            }

            if (beaconsEnabled) {
                drawBeaconsLayer(ctx, layouts, palette, glow, t)
                drawAircraftWarnings(ctx, layouts, palette, glow, t)
            }
        }
    },
    {
        author: 'Hypercolor',
        description:
            'A neon skyline breathes after dark — a moon bleeds haze across endless towers, windows flicker like a population pulse, transit vehicles knife through rain with white-hot headlights, and aircraft warning beacons pulse on every spire',
        presets: [
            {
                controls: {
                    beacons: true,
                    colorMode: 'Blade Runner',
                    glow: 68,
                    haze: 58,
                    scene: 'Skyline',
                    speed: 3,
                    trafficFlow: 48,
                    windowDensity: 64,
                },
                description:
                    'Advertising blimps drift above the Tyrell pyramid — a vast amber sun burns behind teal-black towers as hover-car trails weave the canyon between slabs',
                name: 'Off-World Skyline',
            },
            {
                controls: {
                    beacons: true,
                    colorMode: 'Neo Tokyo',
                    glow: 88,
                    haze: 72,
                    scene: 'Arcology',
                    speed: 6,
                    trafficFlow: 92,
                    windowDensity: 96,
                },
                description:
                    'The intersection at peak chaos — magenta vapor pooling between tower-blocks, every window a story, speeders screaming between levels of the mega-complex',
                name: 'Shibuya Crossing',
            },
            {
                controls: {
                    beacons: true,
                    colorMode: 'Akira',
                    glow: 78,
                    haze: 74,
                    scene: 'Rain Grid',
                    speed: 5,
                    trafficFlow: 64,
                    windowDensity: 58,
                },
                description:
                    'Emergency sirens reflected in black rain — crimson haze bleeding down neo-Tokyo streets, something about to detonate at the center of the frame',
                name: 'Neo Tokyo, 3:14 AM',
            },
            {
                controls: {
                    beacons: true,
                    colorMode: 'Outrun',
                    glow: 72,
                    haze: 62,
                    scene: 'Skyline',
                    speed: 4,
                    trafficFlow: 54,
                    windowDensity: 56,
                },
                description:
                    'The Pacific Coast Highway remembered through a VHS tape — purple dusk melting into hot pink, a convertible slicing through a memory of summer',
                name: 'Pacific Coast Memory',
            },
            {
                controls: {
                    beacons: true,
                    colorMode: 'Ion Chill',
                    glow: 42,
                    haze: 32,
                    scene: 'Skyline',
                    speed: 2,
                    trafficFlow: 36,
                    windowDensity: 72,
                },
                description:
                    'Corporate district, Monday pre-dawn — cold cyan glass walls stare back at themselves, a single amber window means someone has been up all night',
                name: 'Winter Financial District',
            },
            {
                controls: {
                    beacons: true,
                    colorMode: 'Bioluminescent',
                    glow: 64,
                    haze: 58,
                    scene: 'Rain Grid',
                    speed: 3,
                    trafficFlow: 48,
                    windowDensity: 68,
                },
                description:
                    'An alien metropolis grown from coral — organic towers pulse with phosphorescent green, violet pollen drifts through warm tropical rain',
                name: 'Lagoon Citadel',
            },
            {
                controls: {
                    beacons: true,
                    colorMode: 'Mars Colony',
                    glow: 55,
                    haze: 66,
                    scene: 'Skyline',
                    speed: 3,
                    trafficFlow: 38,
                    windowDensity: 52,
                },
                description:
                    'A dust storm clears over Olympus Mons — rust-red haze veils the colony as cobalt oxygen-domes glow against a Martian dawn',
                name: 'Olympus Observatory',
            },
            {
                controls: {
                    beacons: true,
                    colorMode: 'Hollow Moon',
                    glow: 50,
                    haze: 45,
                    scene: 'Skyline',
                    speed: 2,
                    trafficFlow: 26,
                    windowDensity: 38,
                },
                description:
                    'A lunar habitat hours before sunrise — azure moonlight silvering the dome glass, one crimson warning strobe pulsing on every spire',
                name: 'Lunar Nightshift',
            },
            {
                controls: {
                    beacons: false,
                    colorMode: 'Wasabi',
                    glow: 92,
                    haze: 82,
                    scene: 'Arcology',
                    speed: 7,
                    trafficFlow: 86,
                    windowDensity: 88,
                },
                description:
                    'Containment failure in the research arcology — acid-green vapor seeping from every joint as hot-pink warning traffic swarms the affected levels',
                name: 'Biohazard Tower',
            },
            {
                controls: {
                    beacons: true,
                    colorMode: 'Void Matter',
                    glow: 14,
                    haze: 10,
                    scene: 'Skyline',
                    speed: 2,
                    trafficFlow: 16,
                    windowDensity: 18,
                },
                description:
                    'A rolling blackout crawls across the grid — towers winking out floor by floor, only spire warnings and emergency transit still burning through the dark',
                name: 'Blackout Protocol',
            },
            {
                controls: {
                    beacons: false,
                    colorMode: 'Neo Tokyo',
                    glow: 100,
                    haze: 100,
                    scene: 'Rain Grid',
                    speed: 8,
                    trafficFlow: 100,
                    windowDensity: 100,
                },
                description:
                    'Every circuit lit, every lane screaming — the megacity grid maxed out and hallucinating in magenta overload, rain reduced to vertical scan lines',
                name: 'Dopamine Rush',
            },
        ],
    },
)
