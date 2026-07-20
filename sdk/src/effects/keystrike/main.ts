import type { DrawFn } from 'hypercolor'
import { canvas, combo, getInputData, keyToGridPosition, num, pressEnvelope, samplePalette } from 'hypercolor'

const PALETTES = ['SilkCircuit', 'Aurora', 'Cyberpunk', 'Vaporwave', 'Fire', 'Ice', 'Ocean', 'Sunset']

/** A single expanding chromatic ring spawned by a key press. */
interface Ripple {
    x: number
    y: number
    born: number
    hue: number
    strength: number
}

/** A radial shockwave spawned by a pointer click. */
interface Shockwave {
    x: number
    y: number
    born: number
    hue: number
}

const MAX_RIPPLES = 96
const MAX_SHOCKWAVES = 24

/**
 * LED-safe additive plot: accumulate light into the canvas without letting
 * any channel wash to pure white. Colors keep their chroma because we cap
 * the added luminance rather than clamping post-sum.
 */
function ledColor(rgb: [number, number, number], intensity: number, brightness: number): string {
    const gain = Math.max(0, Math.min(1, intensity)) * brightness
    // Gamma-lift so low-intensity tails still register on real LEDs.
    const lifted = gain ** 0.75
    const r = Math.round(rgb[0] * 255 * lifted)
    const g = Math.round(rgb[1] * 255 * lifted)
    const b = Math.round(rgb[2] * 255 * lifted)
    const alpha = Math.max(0.04, Math.min(1, lifted))
    return `rgba(${r},${g},${b},${alpha.toFixed(3)})`
}

function easeOutQuint(t: number): number {
    const clamped = Math.max(0, Math.min(1, t))
    const inv = 1 - clamped
    return 1 - inv * inv * inv * inv * inv
}

export default canvas(
    'Keystrike',
    {
        palette: combo('Palette', PALETTES, {
            default: 'SilkCircuit',
            group: 'Color',
            tooltip: 'Chromatic scheme for ripples, pointer light, and shockwaves.',
        }),
        rippleSpeed: num('Ripple Speed', [1, 10], 5, {
            group: 'Motion',
            step: 0.5,
            tooltip: 'How fast each key ripple expands across the rig.',
        }),
        rippleWidth: num('Ripple Width', [1, 12], 5, {
            group: 'Motion',
            step: 0.5,
            tooltip: 'Thickness of the expanding ring.',
        }),
        decay: num('Decay', [10, 100], 55, {
            group: 'Motion',
            step: 1,
            tooltip: 'How long ripples and shockwaves linger before fading.',
        }),
        idleGlow: num('Idle Glow', [0, 60], 18, {
            group: 'Ambience',
            step: 1,
            tooltip: 'Ambient breathing brightness when nothing is being pressed.',
        }),
        brightness: num('Brightness', [10, 100], 80, {
            group: 'Ambience',
            step: 1,
            tooltip: 'Master output ceiling.',
        }),
    },
    () => {
        const ripples: Ripple[] = []
        const shockwaves: Shockwave[] = []
        const envelope = pressEnvelope({ attackMs: 18, decayMs: 420 })
        let hueOffset = 0
        let trail = 0

        const draw: DrawFn = (ctx, time, controls) => {
            const W = ctx.canvas.width
            const H = ctx.canvas.height
            const paletteName = controls.palette as string
            // num() controls arrive as raw authoring values (only a control
            // named exactly "speed" gets magic-normalized), so read them raw.
            const rippleSpeed = controls.rippleSpeed as number // [1, 10]
            const rippleWidth = controls.rippleWidth as number // [1, 12] px
            const decay = controls.decay as number // [10, 100]
            const idleGlow = controls.idleGlow as number // [0, 60]
            const brightness = (controls.brightness as number) / 100 // [10,100] → [0.1,1]

            const input = getInputData()
            envelope.feed(input.keyboard.events, performanceNow(input))

            // Wheel rotates the palette phase so scrolling recolors the rig.
            hueOffset = (hueOffset + input.mouse.wheel * 0.04) % 1
            if (hueOffset < 0) hueOffset += 1

            const lifeSeconds = 0.6 + (decay / 100) * 2.4
            const ringSpeed = 0.12 + (rippleSpeed / 10) * 0.9

            // Spawn a ripple per key press, positioned by its QWERTY cell so
            // keyboards read as spatially correct and strips still shimmer.
            for (const event of input.keyboard.events) {
                if (event.state !== 'pressed') continue
                const grid = keyToGridPosition(event.key)
                const gx = grid ? grid.x : 0.5
                const gy = grid ? grid.y : 0.5
                ripples.push({
                    x: gx * W,
                    y: gy * H,
                    born: time,
                    hue: (hueOffset + gx * 0.5 + gy * 0.2) % 1,
                    strength: 1,
                })
                if (ripples.length > MAX_RIPPLES) ripples.shift()
            }

            // Pointer clicks throw a brighter shockwave from the cursor.
            for (const event of input.mouse.events) {
                if (event.kind !== 'button' || event.state !== 'pressed') continue
                shockwaves.push({
                    x: input.mouse.nx * W,
                    y: input.mouse.ny * H,
                    born: time,
                    hue: (hueOffset + 0.5) % 1,
                })
                if (shockwaves.length > MAX_SHOCKWAVES) shockwaves.shift()
            }

            // Motion energy feeds a persistent pointer trail glow.
            trail = Math.max(trail * 0.9, Math.min(1, input.mouse.velocity * 0.6))

            const maxRadius = Math.hypot(W, H)

            // ── Base wash: dark, with a slow idle breath so the rig is never
            // fully black while an interactive effect is selected. ──────────
            const breath = 0.5 + 0.5 * Math.sin(time * 1.4)
            const idle = (idleGlow / 60) * breath
            const idleColor = samplePalette(paletteName, hueOffset)
            ctx.globalCompositeOperation = 'source-over'
            ctx.fillStyle = ledColor(idleColor, idle * 0.35, brightness)
            ctx.fillRect(0, 0, W, H)

            ctx.globalCompositeOperation = 'lighter'

            // ── Key ripples ─────────────────────────────────────────────────
            for (const ripple of ripples) {
                const age = (time - ripple.born) / lifeSeconds
                if (age >= 1) continue
                const radius = maxRadius * ringSpeed * (time - ripple.born)
                const fade = 1 - easeOutQuint(age)
                const rgb = samplePalette(paletteName, (ripple.hue + hueOffset) % 1)
                ctx.strokeStyle = ledColor(rgb, fade * ripple.strength, brightness)
                ctx.lineWidth = rippleWidth * (0.5 + fade)
                ctx.beginPath()
                ctx.arc(ripple.x, ripple.y, Math.max(1, radius), 0, Math.PI * 2)
                ctx.stroke()
            }
            pruneExpired(ripples, time, lifeSeconds)

            // ── Click shockwaves (faster, brighter, filled falloff) ─────────
            for (const wave of shockwaves) {
                const age = (time - wave.born) / (lifeSeconds * 0.7)
                if (age >= 1) continue
                const radius = maxRadius * 0.6 * easeOutQuint(age)
                const fade = 1 - age
                const rgb = samplePalette(paletteName, (wave.hue + hueOffset) % 1)
                const gradient = ctx.createRadialGradient(wave.x, wave.y, radius * 0.6, wave.x, wave.y, radius)
                gradient.addColorStop(0, ledColor(rgb, 0, brightness))
                gradient.addColorStop(0.8, ledColor(rgb, fade * 0.8, brightness))
                gradient.addColorStop(1, ledColor(rgb, 0, brightness))
                ctx.fillStyle = gradient
                ctx.beginPath()
                ctx.arc(wave.x, wave.y, radius, 0, Math.PI * 2)
                ctx.fill()
            }
            pruneExpired(shockwaves, time, lifeSeconds * 0.7)

            // ── Pointer light: a roaming source, brighter while it moves ────
            if (input.mouse.available) {
                const px = input.mouse.nx * W
                const py = input.mouse.ny * H
                const glow = 0.35 + trail * 0.65 + (input.mouse.down ? 0.3 : 0)
                const radius = Math.max(W, H) * (0.08 + trail * 0.12)
                const rgb = samplePalette(paletteName, (hueOffset + 0.15) % 1)
                const gradient = ctx.createRadialGradient(px, py, 0, px, py, radius)
                gradient.addColorStop(0, ledColor(rgb, glow, brightness))
                gradient.addColorStop(1, ledColor(rgb, 0, brightness))
                ctx.fillStyle = gradient
                ctx.beginPath()
                ctx.arc(px, py, radius, 0, Math.PI * 2)
                ctx.fill()
            }

            // ── Per-key press glow: recently struck keys pulse at their cell ─
            for (const key of Object.keys(input.keyboard.keys)) {
                const value = envelope.value(key)
                if (value <= 0.01) continue
                const grid = keyToGridPosition(key)
                if (!grid) continue
                const rgb = samplePalette(paletteName, (hueOffset + grid.x * 0.4) % 1)
                const radius = Math.max(W, H) * 0.06 * (0.6 + value)
                const gradient = ctx.createRadialGradient(grid.x * W, grid.y * H, 0, grid.x * W, grid.y * H, radius)
                gradient.addColorStop(0, ledColor(rgb, value, brightness))
                gradient.addColorStop(1, ledColor(rgb, 0, brightness))
                ctx.fillStyle = gradient
                ctx.beginPath()
                ctx.arc(grid.x * W, grid.y * H, radius, 0, Math.PI * 2)
                ctx.fill()
            }

            ctx.globalCompositeOperation = 'source-over'
        }
        return draw
    },
    {
        input: true,
        category: 'interactive',
        description:
            'Every keypress throws a chromatic ripple from its keyboard position; the mouse is a roaming light, clicks burst shockwaves, and the wheel rotates the palette. Idles as a slow breath so the rig never goes dark.',
        presets: [
            {
                controls: {
                    palette: 'SilkCircuit',
                    rippleSpeed: 5,
                    rippleWidth: 5,
                    decay: 55,
                    idleGlow: 18,
                    brightness: 80,
                },
                description: 'Electric purple and cyan ripples on a slow violet breath — the house look.',
                name: 'SilkCircuit',
            },
            {
                controls: { palette: 'Fire', rippleSpeed: 8, rippleWidth: 3, decay: 35, idleGlow: 8, brightness: 90 },
                description: 'Fast, tight embers that snap out from each key and fade quickly.',
                name: 'Ember Typist',
            },
            {
                controls: { palette: 'Ice', rippleSpeed: 3, rippleWidth: 9, decay: 85, idleGlow: 26, brightness: 70 },
                description: 'Wide, slow glacial rings that linger and overlap into aurora sheets.',
                name: 'Glacial Bloom',
            },
        ],
    },
)

/** Milliseconds on the input capture clock, for envelope decay on quiet frames. */
function performanceNow(input: ReturnType<typeof getInputData>): number | undefined {
    const last = input.keyboard.events.at(-1) ?? input.mouse.events.at(-1)
    return last?.atMs
}

function pruneExpired(items: { born: number }[], time: number, life: number): void {
    for (let i = items.length - 1; i >= 0; i--) {
        if (time - items[i].born > life) items.splice(i, 1)
    }
}
