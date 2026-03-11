import { canvas, color, combo, num } from '@hypercolor/sdk'

const TAU = Math.PI * 2

function hexToHSL(hex: string): [number, number, number] {
    const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex)
    if (!result) return [0, 0, 0]

    const r = parseInt(result[1], 16) / 255
    const g = parseInt(result[2], 16) / 255
    const b = parseInt(result[3], 16) / 255

    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    let h = 0
    let s = 0
    const l = (max + min) / 2

    if (max !== min) {
        const d = max - min
        s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
        switch (max) {
            case r:
                h = (g - b) / d + (g < b ? 6 : 0)
                break
            case g:
                h = (b - r) / d + 2
                break
            case b:
                h = (r - g) / d + 4
                break
        }
        h /= 6
    }

    return [Math.round(360 * h), Math.round(s * 100), Math.round(l * 100)]
}

export default canvas(
    'Swirl Reactor',
    {
        backColor: color('Background Color', '#050505'),
        color1: color('Color 1', '#f100ff'),
        color2: color('Color 2', '#00ffd2'),
        color3: color('Color 3', '#0000ff'),
        colorMode: combo('Color Mode', ['Color Cycle', 'Custom', 'Rainbow'], {
            default: 'Custom',
        }),
        cycleSpeed: num('Color Cycle Speed', [0, 100], 50),
        effectRotate: num('Rotation Speed', [0, 100], 50),
        particleGrowth: num('Particle Growth', [-100, 100], 100),
        particleSize: num('Particle Size', [0, 100], 10),
        particleSpawn: num('Particle Amount', [0, 100], 50),
        particleSpeed: num('Particle Speed', [0, 100], 50),
        rotationMode: combo('Rotation Mode', ['Pulse', 'Regular', 'Reverse'], {
            default: 'Regular',
        }),
        spiralAmount: num('Spiral Amount', [1, 3], 3),
    },
    (ctx, time, controls) => {
        const W = ctx.canvas.width
        const H = ctx.canvas.height
        const cx = W / 2
        const cy = H / 2

        const spiralAmount = Math.max(1, Math.round(controls.spiralAmount as number))
        const particleSpeed = controls.particleSpeed as number
        const particleSize = controls.particleSize as number
        const particleGrowth = controls.particleGrowth as number
        const effectRotate = controls.effectRotate as number
        const rotationMode = controls.rotationMode as string
        const colorMode = controls.colorMode as string
        const cycleSpeed = controls.cycleSpeed as number
        const particleSpawn = controls.particleSpawn as number

        const colorArr = [
            hexToHSL(controls.color1 as string),
            hexToHSL(controls.color2 as string),
            hexToHSL(controls.color3 as string),
        ]

        ctx.fillStyle = controls.backColor as string
        ctx.fillRect(0, 0, W, H)

        // Original's per-frame rates (designed for 60fps)
        const movePerFrame = particleSpeed / 50
        const rotPerFrame = effectRotate / 1000
        const growPerFrame = particleGrowth / 200

        if (movePerFrame < 0.001) return

        // Dot spacing along each arm
        const spawnInterval = Math.max(1, 25 - particleSpawn / 4)
        const dotSpacing = Math.max(1.5, movePerFrame * spawnInterval)

        // Current rotation derived purely from time
        const frameTime = time * 60
        const pulseAmp = spiralAmount === 1 ? Math.PI : Math.PI / 2
        const currentRotation =
            rotationMode === 'Pulse'
                ? Math.sin(time * 2) * pulseAmp
                : rotationMode === 'Reverse'
                  ? -frameTime * rotPerFrame
                  : frameTime * rotPerFrame

        // Flowing offset — slides dots outward continuously
        const offset = (frameTime % spawnInterval) * movePerFrame

        // Max visible distance (+ lifetime limit for negative growth)
        const maxDist = Math.hypot(W, H) / 2 + 30
        const lifeDist =
            growPerFrame < 0 ? Math.min(maxDist, ((particleSize / 2 - 1) / -growPerFrame) * movePerFrame) : maxDist

        // Draw far → near so inner (newer) dots render on top
        const farthest = offset + Math.floor((lifeDist - offset) / dotSpacing) * dotSpacing

        // Iterate by distance (far → near), all arms at each distance.
        // This interleaves arms so no single color dominates.
        for (let d = farthest; d >= offset - 0.001; d -= dotSpacing) {
            const framesAgo = d / movePerFrame

            const rad = particleSize / 2 + growPerFrame * framesAgo
            if (rad < 1) continue

            for (let arm = 0; arm < spiralAmount; arm++) {
                const armAngle = (arm * TAU) / spiralAmount
                const c = colorArr[arm] ?? colorArr[0]

                let angle: number
                if (rotationMode === 'Pulse') {
                    angle = Math.sin((time - framesAgo / 60) * 2) * pulseAmp + armAngle
                } else if (rotationMode === 'Reverse') {
                    angle = currentRotation + rotPerFrame * framesAgo + armAngle
                } else {
                    angle = currentRotation - rotPerFrame * framesAgo + armAngle
                }

                let hue = c[0]
                if (colorMode === 'Rainbow') {
                    hue += (frameTime - framesAgo) * 15
                } else if (colorMode === 'Color Cycle') {
                    hue += ((frameTime - framesAgo) * cycleSpeed) / 50
                }

                const px = cx + Math.cos(angle) * d
                const py = cy + Math.sin(angle) * d

                ctx.beginPath()
                ctx.fillStyle = `hsl(${hue}, ${c[1]}%, ${c[2]}%)`
                ctx.arc(px, py, rad, 0, TAU)
                ctx.fill()
            }
        }
    },
    {
        author: 'Hypercolor',
        description: 'So colorful you just want to eat it',
    },
)
