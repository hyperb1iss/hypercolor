import type { FaceContext, FaceDataSources, MediaInfo } from '@hypercolor/sdk'
import {
    arcGauge,
    clamp,
    color,
    face,
    font,
    lerpColor,
    palette,
    Smoothed,
    toggle,
    withAlpha,
} from '@hypercolor/sdk'
import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    resolveFaceInk,
    UI_FONT_FAMILIES,
} from '../shared/dom'

const STYLE_ID = 'hc-face-now-playing'
const ART_FADE_SECS = 0.6
const MARQUEE_SPEED_PX = 36
const MARQUEE_DWELL_SECS = 1.6

const STYLES = `
.hc-now {
    --accent: ${palette.electricPurple};
    --secondary: ${palette.coral};
    --hero-font: 'Sora', sans-serif;
    --ui-font: 'Inter', sans-serif;
    --hero-ink: ${palette.fg.primary};
    --ui-ink: ${palette.fg.secondary};
    --dim-ink: ${palette.fg.tertiary};
    position: absolute;
    inset: 0;
    overflow: hidden;
    color: var(--hero-ink);
    display: flex;
    align-items: center;
    justify-content: center;
}

.hc-now__stack {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 14px;
    width: 86%;
}

.hc-now__art-wrap {
    position: relative;
    border-radius: 50%;
    overflow: hidden;
    background: rgba(255,255,255,0.04);
    box-shadow: 0 10px 36px rgba(0,0,0,0.45);
    flex: 0 0 auto;
}

.hc-now__art {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
}

.hc-now__art--back { z-index: 1; }
.hc-now__art--front { z-index: 2; }

.hc-now__glyph {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 3;
    font-family: var(--hero-font);
    font-weight: 600;
    color: var(--dim-ink);
}

.hc-now__title {
    font-family: var(--hero-font);
    font-weight: 600;
    line-height: 1.1;
    color: var(--hero-ink);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-shadow: 0 0 22px color-mix(in srgb, var(--accent) 18%, transparent);
}

.hc-now__artist {
    font-family: var(--ui-font);
    font-weight: 600;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--ui-ink);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.hc-now__times {
    display: flex;
    flex-direction: row;
    justify-content: space-between;
    width: 100%;
    font-family: var(--ui-font);
    font-weight: 600;
    color: var(--dim-ink);
    font-variant-numeric: tabular-nums lining-nums;
    font-feature-settings: 'tnum' 1, 'lnum' 1;
}

/* ── Wide strip layout ── */

.hc-now--wide .hc-now__stack {
    flex-direction: row;
    align-items: center;
    text-align: left;
    width: 94%;
    height: 78%;
    gap: 0;
}

.hc-now--wide .hc-now__art-wrap {
    border-radius: 14px;
    height: 100%;
    aspect-ratio: 1;
}

.hc-now--wide .hc-now__body {
    display: flex;
    flex-direction: column;
    justify-content: center;
    flex: 1 1 auto;
    min-width: 0;
    gap: 8px;
    margin-left: 4%;
}

.hc-now--wide .hc-now__marquee {
    overflow: hidden;
    width: 100%;
}

.hc-now--wide .hc-now__title {
    display: inline-block;
    text-overflow: clip;
    will-change: transform;
}

.hc-now__hidden { display: none !important; }
`

interface ArtCrossfade {
    setArt(url: string | null, time: number): void
    tick(time: number): void
}

function createArtCrossfade(front: HTMLImageElement, back: HTMLImageElement): ArtCrossfade {
    let currentUrl: string | null = null
    let fadeStartedAt = Number.NEGATIVE_INFINITY

    return {
        setArt(url, time) {
            if (url === currentUrl) return
            if (currentUrl) {
                back.src = currentUrl
                back.classList.remove('hc-now__hidden')
            } else {
                back.classList.add('hc-now__hidden')
            }
            if (url) {
                front.src = url
                front.classList.remove('hc-now__hidden')
            } else {
                front.classList.add('hc-now__hidden')
            }
            currentUrl = url
            fadeStartedAt = time
        },
        tick(time) {
            const progress = clamp01((time - fadeStartedAt) / ART_FADE_SECS)
            front.style.opacity = `${progress}`
            if (progress >= 1) back.classList.add('hc-now__hidden')
        },
    }
}

/** Average the bright pixels of loaded album art into a usable accent. */
function createArtAccentSampler(image: HTMLImageElement) {
    let sampledUrl = ''
    let artAccent: string | null = null

    const sample = (): void => {
        try {
            const probe = document.createElement('canvas')
            probe.width = 12
            probe.height = 12
            const probeCtx = probe.getContext('2d')
            if (!probeCtx) return
            probeCtx.drawImage(image, 0, 0, 12, 12)
            const pixels = probeCtx.getImageData(0, 0, 12, 12).data
            let r = 0
            let g = 0
            let b = 0
            let count = 0
            for (let index = 0; index < pixels.length; index += 4) {
                const pr = pixels[index] ?? 0
                const pg = pixels[index + 1] ?? 0
                const pb = pixels[index + 2] ?? 0
                if (pr + pg + pb < 90) continue
                r += pr
                g += pg
                b += pb
                count += 1
            }
            if (count === 0) return
            const hex = (value: number) =>
                Math.round(clamp(value / count, 0, 255))
                    .toString(16)
                    .padStart(2, '0')
            artAccent = `#${hex(r)}${hex(g)}${hex(b)}`
        } catch {
            artAccent = null
        }
    }

    return (): string | null => {
        const url = image.src
        if (!url || image.classList.contains('hc-now__hidden')) return null
        if (url !== sampledUrl) {
            sampledUrl = url
            artAccent = null
            if (image.complete && image.naturalWidth > 0) {
                sample()
            } else {
                image.addEventListener('load', sample, { once: true })
            }
        }
        return artAccent
    }
}

function formatTrackTime(ms: number): string {
    const totalSeconds = Math.max(0, Math.floor(ms / 1000))
    const minutes = Math.floor(totalSeconds / 60)
    const seconds = totalSeconds % 60
    return `${minutes}:${seconds.toString().padStart(2, '0')}`
}

/** Triangle-wave marquee offset with dwell at both ends. */
function marqueeOffset(time: number, overflowPx: number): number {
    if (overflowPx <= 0) return 0
    const travelSecs = overflowPx / MARQUEE_SPEED_PX
    const period = 2 * (travelSecs + MARQUEE_DWELL_SECS)
    const phase = time % period
    const half = period / 2
    const within = phase < half ? phase : period - phase
    const progress = clamp01((within - MARQUEE_DWELL_SECS / 2) / travelSecs)
    return -overflowPx * progress
}

export default face(
    'Now Playing',
    {
        accent: color('Accent', palette.electricPurple, { group: 'Style' }),
        heroFont: font('Title Font', 'Sora', { families: [...DISPLAY_FONT_FAMILIES], group: 'Typography' }),
        secondaryAccent: color('Secondary', palette.coral, { group: 'Style' }),
        showProgress: toggle('Show Progress', true, { group: 'Elements' }),
        showTimes: toggle('Show Times', true, { group: 'Elements' }),
        uiFont: font('UI Font', 'Inter', { families: [...UI_FONT_FAMILIES], group: 'Typography' }),
        useArtAccent: toggle('Accent From Art', true, { group: 'Style' }),
    },
    {
        author: 'Hypercolor',
        description: 'Album art, orbiting progress, and track info from your media player.',
        designBasis: { height: 480, width: 480 },
        media: true,
        presets: [
            {
                controls: { accent: palette.electricPurple, secondaryAccent: palette.coral, useArtAccent: true },
                description: 'Album-art accents over the SilkCircuit base.',
                name: 'Gallery',
            },
            {
                controls: { accent: palette.neonCyan, secondaryAccent: palette.electricPurple, useArtAccent: false },
                description: 'Fixed cyan/purple chrome regardless of artwork.',
                name: 'Signal',
            },
            {
                controls: {
                    accent: '#ffb347',
                    secondaryAccent: '#ff6b6b',
                    showTimes: false,
                    useArtAccent: false,
                },
                description: 'Warm minimal: art, title, progress.',
                name: 'Ember',
            },
        ],
        variants: {
            wide: (ctx: FaceContext) => buildNowPlaying(ctx, true),
        },
    },
    (ctx) => buildNowPlaying(ctx, false),
)

function buildNowPlaying(ctx: FaceContext, wide: boolean) {
    ensureFaceStyles(STYLE_ID, STYLES)
    const root = createFaceRoot(ctx, 'hc-now')
    root.classList.toggle('hc-now--wide', wide)

    const safe = ctx.display.safeArea
    const artSize = wide ? ctx.height * 0.78 : Math.min(safe.width, safe.height) * 0.46

    root.innerHTML = wide
        ? `
        <div class="hc-now__stack">
            <div class="hc-now__art-wrap">
                <img class="hc-now__art hc-now__art--back hc-now__hidden" alt="" />
                <img class="hc-now__art hc-now__art--front hc-now__hidden" alt="" />
                <div class="hc-now__glyph">&#9835;</div>
            </div>
            <div class="hc-now__body">
                <div class="hc-now__marquee"><div class="hc-now__title">Nothing Playing</div></div>
                <div class="hc-now__artist">waiting for a player</div>
                <div class="hc-now__times"><span class="hc-now__elapsed">0:00</span><span class="hc-now__total">0:00</span></div>
            </div>
        </div>`
        : `
        <div class="hc-now__stack">
            <div class="hc-now__art-wrap">
                <img class="hc-now__art hc-now__art--back hc-now__hidden" alt="" />
                <img class="hc-now__art hc-now__art--front hc-now__hidden" alt="" />
                <div class="hc-now__glyph">&#9835;</div>
            </div>
            <div class="hc-now__title">Nothing Playing</div>
            <div class="hc-now__artist">waiting for a player</div>
        </div>`

    const artWrap = root.querySelector<HTMLDivElement>('.hc-now__art-wrap')
    const artFront = root.querySelector<HTMLImageElement>('.hc-now__art--front')
    const artBack = root.querySelector<HTMLImageElement>('.hc-now__art--back')
    const glyphEl = root.querySelector<HTMLDivElement>('.hc-now__glyph')
    const titleEl = root.querySelector<HTMLDivElement>('.hc-now__title')
    const artistEl = root.querySelector<HTMLDivElement>('.hc-now__artist')
    const marqueeEl = root.querySelector<HTMLDivElement>('.hc-now__marquee')
    const elapsedEl = root.querySelector<HTMLSpanElement>('.hc-now__elapsed')
    const totalEl = root.querySelector<HTMLSpanElement>('.hc-now__total')
    if (!artWrap || !artFront || !artBack || !glyphEl || !titleEl || !artistEl) {
        throw new Error('Now Playing face failed to build its DOM')
    }

    artWrap.style.width = `${artSize}px`
    artWrap.style.height = `${artSize}px`
    glyphEl.style.fontSize = `${artSize * 0.42}px`
    titleEl.style.fontSize = `${Math.max(16, artSize * (wide ? 0.24 : 0.17))}px`
    artistEl.style.fontSize = `${Math.max(10, artSize * (wide ? 0.105 : 0.08))}px`
    if (elapsedEl?.parentElement) {
        elapsedEl.parentElement.style.fontSize = `${Math.max(10, artSize * 0.09)}px`
    }

    const crossfade = createArtCrossfade(artFront, artBack)
    const artAccentFor = createArtAccentSampler(artFront)
    const progressGlide = new Smoothed(0, 0.2)
    const idlePulse = new Smoothed(1, 0.4)
    let lastTime = Number.NaN
    let lastTrackKey = ''

    return (
        time: number,
        controls: Record<string, unknown>,
        _sensors: import('@hypercolor/sdk').SensorAccessor,
        _audio: import('@hypercolor/sdk').AudioAccessor,
        data: FaceDataSources,
    ) => {
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time
        const media: MediaInfo = data.media.state()
        const playing = media.available && media.playing

        const artAccent = controls.useArtAccent === true ? artAccentFor() : null
        const baseAccent = controls.accent as string
        const accent = artAccent ? lerpColor(artAccent, baseAccent, 0.25) : baseAccent
        const secondary = controls.secondaryAccent as string
        const ink = resolveFaceInk(accent)

        root.style.setProperty('--accent', accent)
        root.style.setProperty('--secondary', secondary)
        root.style.setProperty('--hero-ink', ink.hero)
        root.style.setProperty('--ui-ink', ink.ui)
        root.style.setProperty('--dim-ink', ink.dim)
        root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
        root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)

        if (media.available) {
            const trackKey = `${media.player}\u{1f}${media.artist}\u{1f}${media.track}`
            if (trackKey !== lastTrackKey) {
                lastTrackKey = trackKey
                titleEl.textContent = media.track || 'Unknown Track'
                artistEl.textContent = media.artist || media.player
            }
            crossfade.setArt(media.artDataUrl, time)
        } else if (lastTrackKey !== '') {
            lastTrackKey = ''
            titleEl.textContent = 'Nothing Playing'
            artistEl.textContent = 'waiting for a player'
            crossfade.setArt(null, time)
        }
        crossfade.tick(time)
        glyphEl.classList.toggle('hc-now__hidden', Boolean(media.available && media.artDataUrl))
        artWrap.style.opacity = media.available && !playing ? '0.6' : '1'

        if (marqueeEl && wide) {
            const overflow = titleEl.scrollWidth - marqueeEl.clientWidth
            titleEl.style.transform = `translateX(${marqueeOffset(time, overflow).toFixed(1)}px)`
        }

        const positionMs = data.media.positionMs()
        if (elapsedEl && totalEl) {
            const showTimes = controls.showTimes === true && media.available
            elapsedEl.textContent = showTimes ? formatTrackTime(positionMs) : ''
            totalEl.textContent = showTimes ? formatTrackTime(media.durationMs) : ''
        }

        const c = ctx.ctx
        c.clearRect(0, 0, ctx.width, ctx.height)

        const progress = progressGlide.update(media.available ? data.media.progress() : 0, dt)
        const breathe = idlePulse.update(media.available ? 0 : 1, dt)

        if (controls.showProgress === true) {
            if (wide) {
                const railHeight = Math.max(3, ctx.height * 0.035)
                const railY = ctx.height - railHeight * 2.4
                const bodyLeft = artSize + ctx.width * 0.03 + (ctx.width - ctx.width * 0.94) / 2
                const railWidth = ctx.width * 0.97 - bodyLeft - (ctx.width - ctx.width * 0.94) / 2
                c.fillStyle = withAlpha(ink.dim, 0.18)
                c.fillRect(bodyLeft, railY, railWidth, railHeight)
                if (media.available) {
                    const fillGradient = c.createLinearGradient(bodyLeft, 0, bodyLeft + railWidth, 0)
                    fillGradient.addColorStop(0, accent)
                    fillGradient.addColorStop(1, secondary)
                    c.fillStyle = fillGradient
                    c.fillRect(bodyLeft, railY, railWidth * clamp01(progress), railHeight)
                }
            } else {
                // The overlay canvas spans the viewport, so element viewport
                // coordinates are canvas coordinates.
                const wrapRect = artWrap.getBoundingClientRect()
                const cx = wrapRect.left + wrapRect.width / 2
                const cy = wrapRect.top + wrapRect.height / 2
                const ringRadius = artSize / 2 + Math.max(8, artSize * 0.07)
                const orbitCx = Number.isFinite(cx) && cx > 0 ? cx : ctx.width / 2
                const orbitCy = Number.isFinite(cy) && cy > 0 ? cy : ctx.height / 2
                arcGauge(c, {
                    cx: orbitCx,
                    cy: orbitCy,
                    fillColor: [accent, secondary],
                    glow: playing ? 0.55 : 0.18,
                    radius: ringRadius,
                    startAngle: -Math.PI / 2,
                    sweep: Math.PI * 2,
                    thickness: Math.max(3.5, artSize * 0.035),
                    trackColor: withAlpha(ink.dim, 0.14),
                    value: media.available ? clamp01(progress) : 0,
                })
            }
        }

        if (!media.available) {
            const pulse = 0.5 + 0.5 * Math.sin(time * 1.4)
            const glowRadius = artSize * (0.62 + 0.05 * pulse) * breathe
            if (glowRadius > 1) {
                const cx = ctx.width / 2
                const cy = wide ? ctx.height / 2 : ctx.height * 0.42
                const gradient = c.createRadialGradient(cx, cy, 0, cx, cy, glowRadius)
                gradient.addColorStop(0, withAlpha(accent, 0.1 + 0.08 * pulse))
                gradient.addColorStop(1, withAlpha(accent, 0))
                c.fillStyle = gradient
                c.fillRect(0, 0, ctx.width, ctx.height)
            }
        }
    }
}
