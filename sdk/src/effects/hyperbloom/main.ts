/**
 * Hyperbloom — Hypercolor's signature default effect.
 *
 * The real Hypercolor mark is the hero — its chrome, gloss, and gradients are
 * the actual brand artwork, embedded as a data URL. Everything around it is
 * procedural light: a breathing bloom halo, slow rotation, god-rays, a beat
 * chromatic split, and brand-colored embers. It is alive on its own — a slow
 * heartbeat drives the bloom, rays, and embers even in silence — and sound
 * amplifies every layer on top.
 *
 * Rendered in Canvas 2D. Glow/ember/ray layers are pre-baked into offscreen
 * sprites once at setup and drawn additively each frame, so Servo's software
 * canvas never pays for a per-frame blur.
 */

import type { DrawFn } from '@hypercolor/sdk'
import { audio, canvas, num } from '@hypercolor/sdk'
import { HYPERCOLOR_MARK } from './logo'

const VOID = '#0a0612'
// Brand triad — petal colors, used for embers.
const TRIAD = ['#e135ff', '#80ffea', '#ff6ac1'] as const

interface Particle {
    x: number
    y: number
    vx: number
    vy: number
    life: number
    max: number
    size: number
    sprite: HTMLCanvasElement
}

function makeDot(colorHex: string): HTMLCanvasElement {
    const d = document.createElement('canvas')
    d.width = 64
    d.height = 64
    const g = d.getContext('2d')
    if (g) {
        const grad = g.createRadialGradient(32, 32, 0, 32, 32, 32)
        grad.addColorStop(0, '#ffffff')
        grad.addColorStop(0.35, colorHex)
        grad.addColorStop(1, 'rgba(0,0,0,0)')
        g.fillStyle = grad
        g.fillRect(0, 0, 64, 64)
    }
    return d
}

function makeRays(colorHex: string): HTMLCanvasElement {
    const size = 512
    const r = document.createElement('canvas')
    r.width = size
    r.height = size
    const g = r.getContext('2d')
    if (g) {
        g.translate(size / 2, size / 2)
        g.globalCompositeOperation = 'lighter'
        const count = 22
        for (let i = 0; i < count; i++) {
            g.rotate((Math.PI * 2) / count)
            const grad = g.createLinearGradient(0, 0, 0, size / 2)
            grad.addColorStop(0, 'rgba(255,255,255,0)')
            grad.addColorStop(0.12, colorHex)
            grad.addColorStop(1, 'rgba(0,0,0,0)')
            g.fillStyle = grad
            g.beginPath()
            g.moveTo(-3, 0)
            g.lineTo(3, 0)
            g.lineTo(1.2, size / 2)
            g.lineTo(-1.2, size / 2)
            g.closePath()
            g.fill()
        }
    }
    return r
}

// Soft-knee compressor — lets stacked additive ('lighter') passes saturate
// toward a ceiling instead of blowing the center out to flat white, which
// reads as a washed blob on physical LEDs.
function knee(x: number, k = 0.7): number {
    return x / (1 + k * x)
}

// Pre-blurred glow sprite: minify hard, then magnify back with smoothing so
// the halo has no hard edges. An edge-free source upscales smoothly even with
// cheap bilinear filtering, so the bloom never shows blocky stair-steps on
// Servo's software canvas.
function makeGlow(source: CanvasImageSource, srcW: number, srcH: number): HTMLCanvasElement {
    const lw = Math.max(1, Math.round(srcW / 10))
    const lh = Math.max(1, Math.round(srcH / 10))
    const low = document.createElement('canvas')
    low.width = lw
    low.height = lh
    const lc = low.getContext('2d')
    if (lc) {
        lc.imageSmoothingEnabled = true
        lc.imageSmoothingQuality = 'high'
        lc.drawImage(source, 0, 0, lw, lh)
    }
    const w = Math.max(1, Math.round(srcW / 3))
    const h = Math.max(1, Math.round(srcH / 3))
    const glow = document.createElement('canvas')
    glow.width = w
    glow.height = h
    const gc = glow.getContext('2d')
    if (gc) {
        gc.imageSmoothingEnabled = true
        gc.imageSmoothingQuality = 'high'
        gc.drawImage(low, 0, 0, w, h)
    }
    return glow
}

canvas(
    'Hyperbloom',
    {
        aberration: num('Aberration', [0, 100], 38, { group: 'Effects' }),
        backgroundGlow: num('Background Glow', [0, 100], 30, { group: 'Scene' }),
        bloom: num('Bloom', [0, 100], 58, { group: 'Glow' }),
        brightness: num('Brightness', [50, 150], 100, { group: 'Scene' }),
        glow: num('Glow', [0, 100], 56, { group: 'Glow' }),
        idleMotion: num('Idle Motion', [0, 100], 36, { group: 'Motion' }),
        liveliness: num('Liveliness', [0, 100], 60, { group: 'Glow' }),
        rays: num('Light Rays', [0, 100], 40, { group: 'Glow' }),
        size: num('Mark Size', [40, 100], 60, { group: 'Scene' }),
        sparks: num('Sparks', [0, 100], 50, { group: 'Effects' }),
        speed: num('Speed', [1, 10], 4, { group: 'Motion', normalize: 'none' }),
        spin: num('Spin', [-100, 100], 12, { group: 'Motion' }),
    },
    () => {
        const img = new Image()
        let ready = false
        let logoSprite: HTMLCanvasElement | null = null
        let glowSprite: HTMLCanvasElement | null = null
        let raySprite: HTMLCanvasElement | null = null
        let bgSprite: HTMLCanvasElement | null = null
        let bgKey = ''
        const dots: HTMLCanvasElement[] = []
        const particles: Particle[] = []
        let lastTime = 0
        let beatPrev = 0
        let spawnAcc = 0

        const buildSprites = (source: CanvasImageSource, srcW: number, srcH: number) => {
            // Cap the cached mark resolution — it's only ever drawn at ~half the
            // 640x480 canvas, so a smaller sprite means a cheaper per-frame
            // downscale on Servo's software canvas with no visible quality loss.
            const scale = Math.min(1, 576 / srcH)
            const w = Math.max(1, Math.round(srcW * scale))
            const h = Math.max(1, Math.round(srcH * scale))
            logoSprite = document.createElement('canvas')
            logoSprite.width = w
            logoSprite.height = h
            const lc = logoSprite.getContext('2d')
            if (lc) {
                lc.imageSmoothingEnabled = true
                lc.imageSmoothingQuality = 'high'
                lc.drawImage(source, 0, 0, w, h)
            }

            // Pre-blurred glow sprite so the additive halo stays smooth when
            // upscaled at draw time instead of going blocky.
            glowSprite = makeGlow(source, w, h)

            raySprite = makeRays('rgba(200,60,255,0.85)')
            for (const hex of TRIAD) dots.push(makeDot(hex))
            ready = true
        }

        img.onload = () => buildSprites(img, img.naturalWidth || 710, img.naturalHeight || 640)
        // If the embedded mark ever fails to decode (e.g. an engine without webp),
        // fall back to a branded glow disc so the default effect still renders
        // instead of leaving the screen empty forever.
        img.onerror = () => {
            const s = 256
            const fb = document.createElement('canvas')
            fb.width = s
            fb.height = s
            const g = fb.getContext('2d')
            if (g) {
                const grad = g.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2)
                grad.addColorStop(0, '#ffffff')
                grad.addColorStop(0.32, '#e135ff')
                grad.addColorStop(0.7, '#80ffea')
                grad.addColorStop(1, 'rgba(0,0,0,0)')
                g.fillStyle = grad
                g.fillRect(0, 0, s, s)
            }
            buildSprites(fb, s, s)
        }
        img.src = HYPERCOLOR_MARK

        const draw: DrawFn = (ctx, time, c) => {
            const cw = ctx.canvas.width
            const ch = ctx.canvas.height
            const cx = cw / 2
            const cy = ch / 2
            // Cap generously (covers the 10fps tier's 100ms frame) so particle
            // physics keep real-time pace at low FPS, while still guarding against
            // multi-second stalls teleporting everything.
            const dt = Math.min(0.12, Math.max(0.001, time - lastTime || 0.016))
            lastTime = time

            // Servo's software canvas can default to nearest-neighbor scaling.
            // Keep smoothing on, but pick quality per layer below: cheap 'low' for
            // the big blurry glow/ray upscales, 'high' only for the crisp mark.
            ctx.imageSmoothingEnabled = true

            const speedN = (c.speed as number) / 4
            const idle = (c.idleMotion as number) / 100
            const glowCtl = (c.glow as number) / 100
            const bloomCtl = (c.bloom as number) / 100
            const liveCtl = (c.liveliness as number) / 100
            const sparkCtl = (c.sparks as number) / 100
            const rayCtl = (c.rays as number) / 100
            const abCtl = (c.aberration as number) / 100
            const bgCtl = (c.backgroundGlow as number) / 100
            const sizeCtl = (c.size as number) / 100
            const spin = (c.spin as number) / 100
            const bright = (c.brightness as number) / 100

            // ── Autonomous heartbeat — Liveliness sets how strongly the mark
            // pulses, blooms, and sheds embers on its own (and how hard it reacts
            // to audio), so every glow/effect control responds even in silence. ──
            const heart = 0.5 + 0.5 * Math.sin(time * 0.85 * speedN)
            const ambient = (0.28 + 0.5 * heart) * (0.3 + 0.7 * liveCtl)

            // ── Audio (scaled by reactivity) ────────────────────────────────────
            const a = audio()
            const level = (a?.level ?? 0) * liveCtl
            const bass = (a?.bass ?? 0) * liveCtl
            const treble = (a?.treble ?? 0) * liveCtl
            const beatPulse = (a?.beatPulse ?? 0) * liveCtl
            const onsetPulse = (a?.onsetPulse ?? 0) * liveCtl
            const swell = (a?.swell ?? 0) * liveCtl
            const beat = a?.beat ?? 0

            // ── Combined energies: idle baseline + audio, soft-kneed so stacked
            // additive passes saturate instead of blowing out to white. ─────────
            const glowE = knee(glowCtl * (0.4 + 0.34 * heart) + 0.7 * level + 0.9 * beatPulse)
            const bloomE = knee(bloomCtl * (0.16 + 0.5 * ambient + 0.95 * beatPulse + 0.4 * level))
            const rayE = knee(rayCtl * (0.14 + 0.45 * ambient + 0.9 * beatPulse + 0.5 * level + 0.45 * onsetPulse))
            // Aberration: a constant split at idle so the control reads at default,
            // bursting on transients. The second offset draw is gated to real
            // energy, so idle never pays for two full-logo draws.
            const abE = abCtl * (0.14 + 0.85 * beatPulse + 0.5 * onsetPulse)

            // ── Background ──────────────────────────────────────────────────────
            ctx.globalCompositeOperation = 'source-over'
            ctx.globalAlpha = 1
            ctx.fillStyle = VOID
            ctx.fillRect(0, 0, cw, ch)
            if (bgCtl > 0) {
                // Cache the radial sprite per canvas size — rebuilding a
                // full-canvas gradient every frame is costly on Servo's
                // software rasterizer. Only the draw alpha varies.
                const key = `${cw}x${ch}`
                if (!bgSprite || bgKey !== key) {
                    bgKey = key
                    bgSprite = document.createElement('canvas')
                    bgSprite.width = cw
                    bgSprite.height = ch
                    const bc = bgSprite.getContext('2d')
                    if (bc) {
                        const rg = bc.createRadialGradient(cx, cy, 0, cx, cy, Math.max(cw, ch) * 0.62)
                        rg.addColorStop(0, 'rgba(118,42,176,1)')
                        rg.addColorStop(0.5, 'rgba(60,24,110,0.5)')
                        rg.addColorStop(1, 'rgba(10,6,18,0)')
                        bc.fillStyle = rg
                        bc.fillRect(0, 0, cw, ch)
                    }
                }
                const bgPulse = 0.1 * bgCtl * (0.7 + 0.5 * heart) + 0.16 * level
                if (bgSprite && bgPulse > 0.001) {
                    ctx.globalAlpha = Math.min(1, bgPulse)
                    ctx.drawImage(bgSprite, 0, 0)
                    ctx.globalAlpha = 1
                }
            }

            if (!ready || !logoSprite || !glowSprite) return

            const aspect = logoSprite.width / logoSprite.height
            const breathe = 1 + idle * 0.04 * (heart - 0.5) * 2 + bass * 0.09 + swell * 0.05 + beatPulse * 0.05
            const baseH = ch * (0.3 + 0.45 * sizeCtl) * breathe
            const baseW = baseH * aspect
            const sway = idle * 0.03 * Math.sin(time * 0.5 * speedN)
            const rot = sway + time * spin * 1.3 * speedN

            // ── God-rays behind the mark (counter-rotating for parallax) ────────
            if (raySprite && rayE > 0.01) {
                ctx.save()
                ctx.translate(cx, cy)
                ctx.rotate(-time * 0.06 * speedN - rot * 0.2)
                ctx.globalCompositeOperation = 'lighter'
                ctx.imageSmoothingQuality = 'low'
                ctx.globalAlpha = Math.min(0.75, rayE * bright)
                const rs = baseH * 1.95
                ctx.drawImage(raySprite, -rs / 2, -rs / 2, rs, rs)
                ctx.restore()
            }

            ctx.save()
            ctx.translate(cx, cy)
            ctx.rotate(rot)

            // ── Bloom halo (additive). Glow drives the tight core halo; Bloom
            // drives the wide outer halo's size and presence. Both read at idle,
            // and the pre-blurred sprite keeps each 'low'-quality pass cheap. ────
            ctx.globalCompositeOperation = 'lighter'
            ctx.imageSmoothingQuality = 'low'
            const coreAmt = glowE * 0.85 + bloomE * 0.15
            if (coreAmt > 0.01) {
                ctx.globalAlpha = Math.min(0.9, coreAmt * bright)
                const gs = 1.26 + 0.32 * bloomE
                const gw = baseW * gs
                const gh = baseH * gs
                ctx.drawImage(glowSprite, -gw / 2, -gh / 2, gw, gh)
            }
            const haloAmt = bloomE * 0.85 + glowE * 0.12
            if (haloAmt > 0.02) {
                ctx.globalAlpha = Math.min(0.6, haloAmt * bright)
                const hs = 1.7 + 0.7 * bloomE
                const w2 = baseW * hs
                const h2 = baseH * hs
                ctx.drawImage(glowSprite, -w2 / 2, -h2 / 2, w2, h2)
            }

            // ── Chromatic split — one faint offset at idle, full split on energy ─
            const ab = abE * baseW * 0.1
            if (ab > 0.4) {
                ctx.globalCompositeOperation = 'lighter'
                ctx.imageSmoothingQuality = 'low'
                ctx.globalAlpha = Math.min(0.5, 0.12 + abE * 0.5)
                ctx.drawImage(logoSprite, -baseW / 2 + ab, -baseH / 2, baseW, baseH)
                if (abE > 0.2) {
                    ctx.drawImage(logoSprite, -baseW / 2 - ab, -baseH / 2 + ab * 0.4, baseW, baseH)
                }
            }

            // ── The mark itself (the one layer that needs crisp resampling) ─────
            ctx.globalCompositeOperation = 'source-over'
            ctx.imageSmoothingQuality = 'high'
            ctx.globalAlpha = Math.min(1, bright * (0.94 + 0.1 * level))
            ctx.drawImage(logoSprite, -baseW / 2, -baseH / 2, baseW, baseH)

            // Additive self-bloom on energy — blooms the colored aura, not the
            // crisp logo: redrawing the white chrome additively would clamp those
            // pixels to flat white. Audio-driven so the mark never washes at idle.
            const selfBloom = bloomCtl * (0.25 * level + 0.8 * beatPulse) + treble * 0.2
            if (selfBloom > 0.01) {
                ctx.globalCompositeOperation = 'lighter'
                ctx.imageSmoothingQuality = 'low'
                ctx.globalAlpha = Math.min(0.7, selfBloom) * bright
                const bs = 1.08
                ctx.drawImage(glowSprite, (-baseW * bs) / 2, (-baseH * bs) / 2, baseW * bs, baseH * bs)
            }
            ctx.restore()

            // ── Embers: idle trickle + audio bursts ─────────────────────────────
            if (dots.length === 3) {
                spawnAcc += dt * sparkCtl * (1.2 + 4.5 * ambient)
                let toSpawn = Math.floor(spawnAcc)
                spawnAcc -= toSpawn
                const rising = beat > 0.5 && beatPrev <= 0.5
                if (rising) toSpawn += Math.round(sparkCtl * 16 * (0.6 + beatPulse) * liveCtl)
                if (onsetPulse > 0.3) toSpawn += Math.round(sparkCtl * 2)
                for (let i = 0; i < toSpawn; i++) {
                    if (particles.length >= 160) break
                    const ang = Math.random() * Math.PI * 2
                    const spd = (0.22 + Math.random() * 0.7) * ch * (0.6 + beatPulse + 0.3 * ambient)
                    const up = Math.cos(ang)
                    const idx = up < -0.34 ? 0 : Math.sin(ang) > 0 ? 2 : 1
                    particles.push({
                        x: cx,
                        y: cy,
                        vx: Math.sin(ang) * spd,
                        vy: -Math.cos(ang) * spd,
                        life: 0,
                        max: 1.3 + Math.random() * 1.9,
                        size: (0.016 + Math.random() * 0.02) * ch,
                        sprite: dots[idx],
                    })
                }
            }
            beatPrev = beat
            if (particles.length > 0) {
                ctx.globalCompositeOperation = 'lighter'
                for (let i = particles.length - 1; i >= 0; i--) {
                    const p = particles[i]
                    p.life += dt
                    if (p.life >= p.max) {
                        particles.splice(i, 1)
                        continue
                    }
                    const t = p.life / p.max
                    p.x += p.vx * dt
                    p.y += p.vy * dt
                    // Cull embers that have flown off-canvas — also rescues any
                    // left in stale coordinates after a canvas resize.
                    if (p.x < -40 || p.x > cw + 40 || p.y < -40 || p.y > ch + 40) {
                        particles.splice(i, 1)
                        continue
                    }
                    // Light, frame-rate-independent drag — embers keep momentum
                    // and travel well out from the trinity before fading.
                    const drag = Math.exp(-0.9 * dt)
                    p.vx *= drag
                    p.vy = p.vy * drag + 10 * dt
                    const fade = (1 - t) * (0.55 + 0.45 * treble)
                    const sz = p.size * (1.25 - 0.75 * t)
                    ctx.globalAlpha = Math.min(1, fade * bright)
                    ctx.drawImage(p.sprite, p.x - sz, p.y - sz, sz * 2, sz * 2)
                }
            }

            ctx.globalCompositeOperation = 'source-over'
            ctx.globalAlpha = 1
        }

        return draw
    },
    {
        audio: true,
        author: 'Hypercolor',
        category: 'ambient',
        description:
            'The Hypercolor mark, alive as light. The real brand artwork turns slowly inside a breathing bloom on void-black, god-rays drifting behind it and embers rising from the trinity. It glows on its own and wakes up with sound: bass swells the bloom, beats split the mark and blast the rays, and embers scatter on every hit.',
        presets: [
            {
                controls: {
                    aberration: 38,
                    backgroundGlow: 30,
                    bloom: 58,
                    brightness: 100,
                    glow: 56,
                    idleMotion: 36,
                    liveliness: 60,
                    rays: 40,
                    size: 60,
                    sparks: 50,
                    speed: 4,
                    spin: 12,
                },
                description:
                    'The balanced brand statement — the mark turning in a soft bloom with embers drifting up, lively the moment sound arrives.',
                name: 'Signature',
            },
            {
                controls: {
                    aberration: 8,
                    backgroundGlow: 16,
                    bloom: 30,
                    brightness: 92,
                    glow: 38,
                    idleMotion: 12,
                    liveliness: 22,
                    rays: 10,
                    size: 52,
                    sparks: 10,
                    speed: 2,
                    spin: 4,
                },
                description:
                    'Barely moving. A small, quiet mark glowing in the dark — calm enough to live behind everything.',
                name: 'Still',
            },
            {
                controls: {
                    aberration: 20,
                    backgroundGlow: 78,
                    bloom: 64,
                    brightness: 104,
                    glow: 80,
                    idleMotion: 56,
                    liveliness: 46,
                    rays: 64,
                    size: 70,
                    sparks: 24,
                    speed: 3,
                    spin: 18,
                },
                description:
                    'Vast and dreamy. The mark floats large in a wide aurora haze, god-rays drifting behind it — slow and atmospheric.',
                name: 'Nebula',
            },
            {
                controls: {
                    aberration: 46,
                    backgroundGlow: 24,
                    bloom: 68,
                    brightness: 102,
                    glow: 54,
                    idleMotion: 30,
                    liveliness: 88,
                    rays: 46,
                    size: 60,
                    sparks: 66,
                    speed: 5,
                    spin: 10,
                },
                description:
                    'Tuned for music. Lively at rest and explosive with sound — bass swells the bloom, beats split the mark, embers scatter.',
                name: 'Pulse',
            },
            {
                controls: {
                    aberration: 82,
                    backgroundGlow: 18,
                    bloom: 94,
                    brightness: 110,
                    glow: 70,
                    idleMotion: 50,
                    liveliness: 100,
                    rays: 90,
                    size: 64,
                    sparks: 100,
                    speed: 8,
                    spin: 30,
                },
                description:
                    'Maxed out. Fast spin, hard bloom, god-rays blasting, embers everywhere. For a room with the bass up.',
                name: 'Rave',
            },
            {
                controls: {
                    aberration: 28,
                    backgroundGlow: 14,
                    bloom: 56,
                    brightness: 98,
                    glow: 96,
                    idleMotion: 22,
                    liveliness: 44,
                    rays: 18,
                    size: 58,
                    sparks: 20,
                    speed: 3,
                    spin: 7,
                },
                description:
                    "Pure chrome. A heavy, steady halo wrapping the mark's liquid-metal rim — molten, minimal, elegant.",
                name: 'Halo',
            },
        ],
    },
)
