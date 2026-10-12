import type { DrawFn } from 'hypercolor'
import { canvas, combo, getInputData, num, samplePalette } from 'hypercolor'

const PALETTES = ['SilkCircuit', 'Aurora', 'Cyberpunk', 'Vaporwave', 'Ocean', 'Ice', 'Fire', 'Sunset']

/** An expanding ring thrown from the point where a contact landed or lifted. */
interface Ripple {
    x: number
    y: number
    born: number
    hue: number
    strength: number
}

/** Last known canvas point of a held contact, kept so its glow fades after the lift. */
interface Contact {
    x: number
    y: number
    pressure: number
    hue: number
    releasedAt: number | null
}

const MAX_RIPPLES = 64
const CONTACT_FADE_S = 0.35
const BUTTON_PHASE_STEP = 1 / 6

/**
 * Additive headroom: one light source tops out here so overlapping rings and
 * contacts can sum under `lighter` compositing without any single one
 * clipping toward white on the LEDs.
 */
const HEADROOM = 0.8

/**
 * Strip most of a palette color's white component, keeping its peak channel,
 * so pastel stops still read as a saturated hue on point-source LEDs.
 */
function vivid([r, g, b]: [number, number, number]): [number, number, number] {
    const max = Math.max(r, g, b)
    const floor = Math.min(r, g, b) * 0.85
    const span = max - floor
    if (span <= 1e-6) return [r, g, b]
    return [((r - floor) / span) * max, ((g - floor) / span) * max, ((b - floor) / span) * max]
}

/**
 * A palette color at `intensity`, carried in alpha so gradients fade to the
 * same hue instead of through grey. The frame is cleared every draw, so
 * nothing accumulates past this ceiling except where lights overlap.
 */
function light(rgb: [number, number, number], intensity: number, brightness: number): string {
    // Lift low intensities so ring tails still register on real LEDs.
    const alpha = Math.max(0, Math.min(1, intensity)) ** 0.8 * brightness * HEADROOM
    const r = Math.round(rgb[0] * 255)
    const g = Math.round(rgb[1] * 255)
    const b = Math.round(rgb[2] * 255)
    return `rgba(${r},${g},${b},${alpha.toFixed(3)})`
}

/** A vivid palette color at `t`, wrapping around the palette's ends. */
function hue(paletteName: string, t: number): [number, number, number] {
    return vivid(samplePalette(paletteName, ((t % 1) + 1) % 1))
}

/** Ring brightness over its life: holds early, then tails off. */
function ringFade(age: number): number {
    return (1 - Math.max(0, Math.min(1, age))) ** 1.4
}

function contactKey(device: string, contact: number): string {
    return `${device}:${contact}`
}

export default canvas(
    'Touch Ripple',
    {
        palette: combo('Palette', PALETTES, {
            default: 'SilkCircuit',
            group: 'Color',
            tooltip: 'Chromatic scheme for ripples and fingertip glow. Device buttons step through it.',
        }),
        rippleSpeed: num('Ripple Speed', [1, 10], 4, {
            group: 'Motion',
            step: 0.5,
            tooltip: 'How fast each ripple spreads out from the touch.',
        }),
        rippleWidth: num('Ripple Width', [1, 10], 5, {
            group: 'Motion',
            step: 0.5,
            tooltip: 'Thickness of the expanding ring.',
        }),
        decay: num('Decay', [10, 100], 50, {
            group: 'Motion',
            step: 1,
            tooltip: 'How long ripples linger before fading.',
        }),
        glow: num('Fingertip Glow', [0, 100], 60, {
            group: 'Touch',
            step: 1,
            tooltip: 'Size of the light that follows each held finger. Pressure makes it bigger and brighter.',
        }),
        idleGlow: num('Idle Glow', [0, 60], 14, {
            group: 'Ambience',
            step: 1,
            tooltip: 'Ambient breathing brightness while nothing is touched.',
        }),
        brightness: num('Brightness', [10, 100], 80, {
            group: 'Ambience',
            step: 1,
            tooltip: 'Master output ceiling.',
        }),
    },
    () => {
        const ripples: Ripple[] = []
        const contacts = new Map<string, Contact>()
        let lastSeq = -1
        let phase = 0

        const draw: DrawFn = (ctx, time, controls) => {
            const W = ctx.canvas.width
            const H = ctx.canvas.height
            const minSide = Math.min(W, H)
            const paletteName = controls.palette as string
            // num() controls arrive as raw authoring values.
            const rippleSpeed = controls.rippleSpeed as number // [1, 10]
            const rippleWidth = controls.rippleWidth as number // [1, 10]
            const decay = controls.decay as number // [10, 100]
            const glowSize = (controls.glow as number) / 100 // [0, 1]
            const idleGlow = controls.idleGlow as number // [0, 60]
            const brightness = (controls.brightness as number) / 100 // [0.1, 1]

            const lifeSeconds = 0.5 + (decay / 100) * 2.5
            const ringSpeed = 0.15 + (rippleSpeed / 10) * 0.85 // canvas diagonals per second
            // Half-width of the ring's soft band. Wide enough that a 15x15
            // pad filling the canvas still sees it span more than one LED.
            const ringWidth = minSide * (0.02 + (rippleWidth / 10) * 0.1)
            const maxRadius = Math.hypot(W, H)

            const input = getInputData()

            // Edges are deduplicated by sequence number, so a page that draws
            // more than once per host frame never doubles a ripple.
            for (const event of input.deviceButtons.events) {
                if (event.seq <= lastSeq) continue
                if (event.state === 'pressed') phase = (phase + BUTTON_PHASE_STEP) % 1
            }
            for (const event of input.touch.events) {
                if (event.seq <= lastSeq) continue
                // A contact on a device with no place in this canvas's layout
                // has no point to ripple from; its LEDs show another canvas.
                if (!event.canvas || event.phase === 'cancelled') continue
                const key = contactKey(event.device, event.contact)
                if (event.phase === 'began') {
                    const hue = (phase + event.canvas.nx * 0.35 + event.canvas.ny * 0.15) % 1
                    ripples.push({
                        born: time,
                        hue,
                        strength: 0.55 + event.pressure * 0.45,
                        x: event.canvas.x,
                        y: event.canvas.y,
                    })
                    contacts.set(key, {
                        hue,
                        pressure: event.pressure,
                        releasedAt: null,
                        x: event.canvas.x,
                        y: event.canvas.y,
                    })
                } else {
                    // The lift throws a softer echo ring from where the finger left.
                    const held = contacts.get(key)
                    ripples.push({
                        born: time,
                        hue: ((held?.hue ?? phase) + 0.08) % 1,
                        strength: 0.35,
                        x: event.canvas.x,
                        y: event.canvas.y,
                    })
                }
                while (ripples.length > MAX_RIPPLES) ripples.shift()
            }
            for (const event of input.touch.events) lastSeq = Math.max(lastSeq, event.seq)
            for (const event of input.deviceButtons.events) lastSeq = Math.max(lastSeq, event.seq)

            // Held contacts follow the finger; anything no longer held starts fading.
            const heldKeys = new Set<string>()
            for (const touch of input.touch.touches) {
                if (!touch.canvas) continue
                const key = contactKey(touch.device, touch.contact)
                heldKeys.add(key)
                const contact = contacts.get(key)
                if (contact) {
                    contact.x = touch.canvas.x
                    contact.y = touch.canvas.y
                    contact.pressure = touch.pressure
                    contact.releasedAt = null
                } else {
                    const hue = (phase + touch.canvas.nx * 0.35 + touch.canvas.ny * 0.15) % 1
                    contacts.set(key, {
                        hue,
                        pressure: touch.pressure,
                        releasedAt: null,
                        x: touch.canvas.x,
                        y: touch.canvas.y,
                    })
                }
            }
            for (const [key, contact] of contacts) {
                if (heldKeys.has(key)) continue
                if (contact.releasedAt === null) contact.releasedAt = time
                if (time - contact.releasedAt > CONTACT_FADE_S) contacts.delete(key)
            }

            // ── Base wash: a dark field with a slow breath, deeper while touched.
            // Every light below is animated on the clock, so the frame starts
            // clean rather than trailing the last one. ──
            const breath = 0.5 + 0.5 * Math.sin(time * 1.6)
            const idle = (idleGlow / 60) * breath * (input.touch.down ? 0.5 : 1)
            ctx.globalCompositeOperation = 'source-over'
            ctx.fillStyle = '#000'
            ctx.fillRect(0, 0, W, H)
            ctx.fillStyle = light(hue(paletteName, phase), idle * 0.3, brightness)
            ctx.fillRect(0, 0, W, H)

            ctx.globalCompositeOperation = 'lighter'

            // ── Ripples ──
            for (const ripple of ripples) {
                const elapsed = time - ripple.born
                const age = elapsed / lifeSeconds
                if (age >= 1) continue
                const radius = maxRadius * ringSpeed * elapsed
                if (radius > maxRadius * 1.1) continue
                const fade = ringFade(age)
                const rgb = hue(paletteName, (ripple.hue + age * 0.12) % 1)
                // A soft band instead of a hard stroke, so the ring glides
                // across sparse LEDs rather than flickering between them.
                const band = ringWidth * (0.6 + fade * 0.6)
                const inner = Math.max(0, radius - band)
                const outer = radius + band
                const gradient = ctx.createRadialGradient(ripple.x, ripple.y, inner, ripple.x, ripple.y, outer)
                gradient.addColorStop(0, light(rgb, 0, brightness))
                gradient.addColorStop(0.5, light(rgb, fade * ripple.strength, brightness))
                gradient.addColorStop(1, light(rgb, 0, brightness))
                ctx.fillStyle = gradient
                ctx.beginPath()
                ctx.arc(ripple.x, ripple.y, outer, 0, Math.PI * 2)
                ctx.fill()
            }
            for (let i = ripples.length - 1; i >= 0; i--) {
                if (time - ripples[i].born >= lifeSeconds) ripples.splice(i, 1)
            }

            // ── Fingertip glow: a hot spot under each finger, swelling with pressure ──
            if (glowSize > 0) {
                for (const contact of contacts.values()) {
                    const release =
                        contact.releasedAt === null ? 1 : Math.max(0, 1 - (time - contact.releasedAt) / CONTACT_FADE_S)
                    const intensity = (0.45 + contact.pressure * 0.55) * release
                    const radius = minSide * glowSize * (0.08 + contact.pressure * 0.14)
                    if (radius < 1 || intensity <= 0) continue
                    const rgb = hue(paletteName, contact.hue)
                    const gradient = ctx.createRadialGradient(contact.x, contact.y, 0, contact.x, contact.y, radius)
                    gradient.addColorStop(0, light(rgb, intensity, brightness))
                    gradient.addColorStop(1, light(rgb, 0, brightness))
                    ctx.fillStyle = gradient
                    ctx.beginPath()
                    ctx.arc(contact.x, contact.y, radius, 0, Math.PI * 2)
                    ctx.fill()
                }
            }

            ctx.globalCompositeOperation = 'source-over'
        }
        return draw
    },
    {
        input: true,
        category: 'interactive',
        description:
            'Touch a ROLI Lightpad or any touch surface Hypercolor drives: each finger lights a glow that swells with pressure and throws a ripple across the rig from exactly where it landed. Device buttons step the palette.',
        presets: [
            {
                controls: {
                    palette: 'SilkCircuit',
                    rippleSpeed: 4,
                    rippleWidth: 5,
                    decay: 50,
                    glow: 60,
                    idleGlow: 14,
                    brightness: 80,
                },
                description: 'Purple and cyan rings over a slow violet breath, the house look.',
                name: 'SilkCircuit',
            },
            {
                controls: {
                    palette: 'Ocean',
                    rippleSpeed: 2.5,
                    rippleWidth: 8,
                    decay: 85,
                    glow: 75,
                    idleGlow: 20,
                    brightness: 70,
                },
                description: 'Slow, wide swells that linger like rain on still water.',
                name: 'Still Water',
            },
            {
                controls: {
                    palette: 'Cyberpunk',
                    rippleSpeed: 8,
                    rippleWidth: 3,
                    decay: 30,
                    glow: 45,
                    idleGlow: 6,
                    brightness: 90,
                },
                description: 'Fast, tight magenta snaps for drumming on the pad.',
                name: 'Finger Drum',
            },
        ],
    },
)
