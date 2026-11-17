import type { FaceContext, FaceDataSources, MediaInfo } from 'hypercolor'
import { arcGauge, clamp, color, face, font, lerpColor, num, palette, Smoothed, toggle, withAlpha } from 'hypercolor'
import { atmosphereVisible, drawNebulaField, transparentBackgroundControl } from '../shared/atmosphere'
import {
    clamp01,
    createFaceRoot,
    DISPLAY_FONT_FAMILIES,
    ensureFaceStyles,
    resolveFaceInk,
    SmoothedColor,
    UI_FONT_FAMILIES,
} from '../shared/dom'
import { layoutStack, layoutStrip, type NowPlayingSizes } from './layout'

const STYLE_ID = 'hc-face-now-playing'
const ART_FADE_SECS = 0.6
/** Marquee travel per second, in title heights. */
const MARQUEE_SPEED_EMS = 1.2
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
    width: 100%;
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

.hc-now__marquee {
    overflow: hidden;
    flex: 0 0 auto;
    text-align: center;
}

.hc-now__marquee--running {
    text-align: left;
    -webkit-mask-image: linear-gradient(to right, transparent, #000 7%, #000 93%, transparent);
    mask-image: linear-gradient(to right, transparent, #000 7%, #000 93%, transparent);
}

.hc-now__title {
    display: inline-block;
    font-family: var(--hero-font);
    font-weight: 600;
    color: var(--hero-ink);
    white-space: nowrap;
    will-change: transform;
    text-shadow: 0 0 22px color-mix(in srgb, var(--accent) 18%, transparent);
}

.hc-now__artist {
    display: inline-block;
    font-family: var(--ui-font);
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ui-ink);
    white-space: nowrap;
    will-change: transform;
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

.hc-now__times--round {
    justify-content: center;
    gap: 0.4em;
    width: auto;
    flex: 0 0 auto;
}

.hc-now__times-sep {
    color: color-mix(in srgb, var(--dim-ink) 55%, transparent);
}

/* ── Wide strip layout ── */

.hc-now--wide .hc-now__stack {
    flex-direction: row;
    align-items: center;
    text-align: left;
    width: 94%;
    gap: 0;
}

.hc-now--wide .hc-now__art-wrap {
    border-radius: 14px;
    margin-right: 4%;
}

.hc-now--wide .hc-now__body {
    display: flex;
    flex-direction: column;
    justify-content: center;
    flex: 1 1 auto;
    min-width: 0;
}

.hc-now--wide .hc-now__marquee {
    width: 100%;
    text-align: left;
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

/** Average the lit pixels of loaded album art into an accent bright enough to read. */
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
            // Keep the art's hue and lift it to full brightness: a dark
            // cover would otherwise ink the title in mud.
            const lift = (255 * count) / Math.max(r, g, b, 1)
            const hex = (value: number) =>
                Math.round(clamp((value / count) * lift, 0, 255))
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
function marqueeOffset(time: number, overflowPx: number, speedPx: number): number {
    if (overflowPx <= 0) return 0
    const travelSecs = overflowPx / speedPx
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
        artSize: num('Art Size', [20, 70], 44, { group: 'Layout', step: 1 }),
        detailSize: num('Detail Size', [10, 72], 40, { group: 'Typography', step: 1 }),
        heroFont: font('Title Font', 'Sora', {
            families: [...UI_FONT_FAMILIES, ...DISPLAY_FONT_FAMILIES.filter((family) => family !== 'Exo 2')],
            group: 'Typography',
            weights: [600],
        }),
        secondaryAccent: color('Secondary', palette.coral, { group: 'Style' }),
        showArt: toggle('Show Art', true, { group: 'Elements' }),
        showProgress: toggle('Show Progress', true, { group: 'Elements' }),
        showTimes: toggle('Show Times', true, { group: 'Elements' }),
        titleSize: num('Title Size', [20, 140], 84, { group: 'Typography', step: 1 }),
        transparentBackground: transparentBackgroundControl(),
        uiFont: font('Detail Font', 'Inter', {
            families: [...UI_FONT_FAMILIES],
            group: 'Typography',
            weights: [600],
        }),
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
            {
                controls: {
                    detailSize: 48,
                    showArt: false,
                    showTimes: false,
                    titleSize: 110,
                },
                description: 'Type only, sized to read from across the room.',
                name: 'Marquee',
            },
        ],
        variants: {
            wide: (ctx: FaceContext) => buildNowPlaying(ctx, true),
        },
    },
    (ctx) => buildNowPlaying(ctx, false),
)

function readSizes(controls: Record<string, unknown>, timesShown: boolean): NowPlayingSizes {
    return {
        artSize: controls.artSize as number,
        detailSize: controls.detailSize as number,
        showArt: controls.showArt === true,
        showProgress: controls.showProgress === true,
        showTimes: timesShown,
        titleSize: controls.titleSize as number,
    }
}

function buildNowPlaying(ctx: FaceContext, wide: boolean) {
    ensureFaceStyles(STYLE_ID, STYLES)
    const root = createFaceRoot(ctx, 'hc-now')
    root.classList.toggle('hc-now--wide', wide)

    const art = `
            <div class="hc-now__art-wrap">
                <img class="hc-now__art hc-now__art--back hc-now__hidden" alt="" />
                <img class="hc-now__art hc-now__art--front hc-now__hidden" alt="" />
                <div class="hc-now__glyph">&#9835;</div>
            </div>`
    const title = `<div class="hc-now__marquee"><div class="hc-now__title">Nothing Playing</div></div>`
    const artist = `<div class="hc-now__marquee hc-now__marquee--detail"><div class="hc-now__artist">waiting for a player</div></div>`
    root.innerHTML = wide
        ? `
        <div class="hc-now__stack">${art}
            <div class="hc-now__body">
                ${title}
                ${artist}
                <div class="hc-now__times"><span class="hc-now__elapsed">0:00</span><span class="hc-now__total">0:00</span></div>
            </div>
        </div>`
        : `
        <div class="hc-now__stack">${art}
            ${title}
            ${artist}
            <div class="hc-now__times hc-now__times--round"><span class="hc-now__elapsed">0:00</span><span class="hc-now__times-sep">/</span><span class="hc-now__total">0:00</span></div>
        </div>`

    const artWrap = root.querySelector<HTMLDivElement>('.hc-now__art-wrap')
    const artFront = root.querySelector<HTMLImageElement>('.hc-now__art--front')
    const artBack = root.querySelector<HTMLImageElement>('.hc-now__art--back')
    const glyphEl = root.querySelector<HTMLDivElement>('.hc-now__glyph')
    const titleEl = root.querySelector<HTMLDivElement>('.hc-now__title')
    const artistEl = root.querySelector<HTMLDivElement>('.hc-now__artist')
    const marqueeEl = root.querySelector<HTMLDivElement>('.hc-now__marquee')
    const artistLineEl = root.querySelector<HTMLDivElement>('.hc-now__marquee--detail')
    const timesEl = root.querySelector<HTMLDivElement>('.hc-now__times')
    const elapsedEl = root.querySelector<HTMLSpanElement>('.hc-now__elapsed')
    const totalEl = root.querySelector<HTMLSpanElement>('.hc-now__total')
    const stackEl = root.querySelector<HTMLDivElement>('.hc-now__stack')
    if (
        !artWrap ||
        !artFront ||
        !artBack ||
        !glyphEl ||
        !titleEl ||
        !artistEl ||
        !marqueeEl ||
        !artistLineEl ||
        !timesEl ||
        !elapsedEl ||
        !totalEl ||
        !stackEl
    ) {
        throw new Error('Now Playing face failed to build its DOM')
    }

    const sizeLine = (element: HTMLElement, fontSize: number, lineHeight: number): void => {
        element.style.fontSize = `${fontSize}px`
        element.style.lineHeight = `${lineHeight}px`
        element.style.height = `${lineHeight}px`
    }
    const sizeArt = (size: number): void => {
        artWrap.classList.toggle('hc-now__hidden', size <= 0)
        artWrap.style.width = `${size}px`
        artWrap.style.height = `${size}px`
        glyphEl.style.fontSize = `${size * 0.42}px`
    }

    /** The art's size, the progress ring or rail, and each line's travel speed. */
    interface Applied {
        art: number
        ring: ReturnType<typeof layoutStack>['ring'] | null
        marqueeSpeed: number
        detailSpeed: number
    }

    const applyStack = (sizes: NowPlayingSizes): Applied => {
        const layout = layoutStack(ctx, sizes)
        stackEl.style.gap = `${layout.gap}px`
        sizeArt(layout.art)
        artWrap.style.margin = `${layout.artOutset}px 0`
        sizeLine(marqueeEl, layout.title.fontSize, layout.title.height)
        marqueeEl.style.width = `${layout.title.width}px`
        sizeLine(artistLineEl, layout.artist.fontSize, layout.artist.height)
        artistLineEl.style.width = `${layout.artist.width}px`
        sizeLine(timesEl, layout.times.fontSize, layout.times.height)
        return {
            art: layout.art,
            detailSpeed: layout.artist.fontSize * MARQUEE_SPEED_EMS,
            marqueeSpeed: layout.title.fontSize * MARQUEE_SPEED_EMS,
            ring: layout.ring,
        }
    }

    const applyStrip = (sizes: NowPlayingSizes): Applied => {
        const layout = layoutStrip(ctx, sizes)
        sizeArt(layout.art)
        marqueeEl.style.fontSize = `${layout.titleSize}px`
        marqueeEl.style.lineHeight = '1.16'
        artistLineEl.style.fontSize = `${layout.artistSize}px`
        artistLineEl.style.lineHeight = '1.3'
        artistLineEl.style.marginTop = `${layout.gap}px`
        timesEl.style.fontSize = `${layout.timesSize}px`
        timesEl.style.lineHeight = '1.3'
        timesEl.style.marginTop = `${layout.gap}px`
        return {
            art: layout.art,
            detailSpeed: layout.artistSize * MARQUEE_SPEED_EMS,
            marqueeSpeed: layout.titleSize * MARQUEE_SPEED_EMS,
            ring: null,
        }
    }

    const crossfade = createArtCrossfade(artFront, artBack)
    const artAccentFor = createArtAccentSampler(artFront)
    const progressGlide = new Smoothed(0, 0.2)
    const idlePulse = new Smoothed(1, 0.4)
    // A longer halflife here so art-derived accents wash in with the fade.
    const accentGlide = new SmoothedColor(palette.electricPurple, 0.18)
    const secondaryGlide = new SmoothedColor(palette.coral)
    let lastTime = Number.NaN
    let lastTrackKey = ''
    let layoutKey = ''
    let applied: Applied = { art: 0, detailSpeed: 0, marqueeSpeed: 0, ring: null }
    let fitPending = true
    let fittedAt = 0

    /**
     * The idle lines are fixed copy, so they shrink to their line instead
     * of travelling. Idle copy is measured again each second, because the
     * first measure can land before the font does and the fallback face
     * has different widths.
     */
    const fitIdleCopy = (idle: boolean, time: number): void => {
        const lines: [HTMLElement, HTMLElement][] = [
            [marqueeEl, titleEl],
            [artistLineEl, artistEl],
        ]
        for (const [line, text] of lines) {
            text.style.fontSize = ''
            if (!idle) continue
            const ratio = line.clientWidth / Math.max(1, text.scrollWidth)
            if (ratio < 1) text.style.fontSize = `${Math.floor(ratio * 100)}%`
        }
        fitPending = false
        fittedAt = time
    }

    return (
        time: number,
        controls: Record<string, unknown>,
        _sensors: import('hypercolor').SensorAccessor,
        _audio: import('hypercolor').AudioAccessor,
        data: FaceDataSources,
    ) => {
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time
        const media: MediaInfo = data.media.state()
        const playing = media.available && media.playing

        const artAccent = controls.useArtAccent === true ? artAccentFor() : null
        const baseAccent = controls.accent as string
        const accent = accentGlide.update(artAccent ? lerpColor(artAccent, baseAccent, 0.25) : baseAccent, dt)
        const secondary = secondaryGlide.update(controls.secondaryAccent as string, dt)
        const ink = resolveFaceInk(accent)

        root.style.setProperty('--accent', accent)
        root.style.setProperty('--secondary', secondary)
        root.style.setProperty('--hero-ink', ink.hero)
        root.style.setProperty('--ui-ink', ink.ui)
        root.style.setProperty('--dim-ink', ink.dim)
        root.style.setProperty('--hero-font', `"${controls.heroFont as string}", sans-serif`)
        root.style.setProperty('--ui-font', `"${controls.uiFont as string}", sans-serif`)

        const showTimes = controls.showTimes === true && media.available
        const sizes = readSizes(controls, showTimes)
        const nextKey = JSON.stringify(sizes)
        if (nextKey !== layoutKey) {
            layoutKey = nextKey
            fitPending = true
            timesEl.classList.toggle('hc-now__hidden', !showTimes)
            applied = wide ? applyStrip(sizes) : applyStack(sizes)
        }

        if (media.available) {
            const trackKey = `${media.player}\u{1f}${media.artist}\u{1f}${media.track}`
            if (trackKey !== lastTrackKey) {
                lastTrackKey = trackKey
                fitPending = true
                titleEl.textContent = media.track || 'Unknown Track'
                artistEl.textContent = media.artist || media.player
            }
            crossfade.setArt(media.artDataUrl, time)
        } else if (lastTrackKey !== '') {
            lastTrackKey = ''
            fitPending = true
            titleEl.textContent = 'Nothing Playing'
            artistEl.textContent = 'waiting for a player'
            crossfade.setArt(null, time)
        }
        crossfade.tick(time)
        glyphEl.classList.toggle('hc-now__hidden', Boolean(media.available && media.artDataUrl))
        artWrap.style.opacity = media.available && !playing ? '0.6' : '1'

        if (fitPending || (!media.available && time - fittedAt > 1)) fitIdleCopy(!media.available, time)

        // A line wider than its room travels end to end instead of clipping.
        // The artist starts half a dwell behind the title so the two lines
        // do not slide in step.
        const overflow = titleEl.scrollWidth - marqueeEl.clientWidth
        marqueeEl.classList.toggle('hc-now__marquee--running', overflow > 0)
        titleEl.style.transform = `translateX(${marqueeOffset(time, overflow, applied.marqueeSpeed).toFixed(1)}px)`
        const artistOverflow = artistEl.scrollWidth - artistLineEl.clientWidth
        artistLineEl.classList.toggle('hc-now__marquee--running', artistOverflow > 0)
        artistEl.style.transform = `translateX(${marqueeOffset(
            Math.max(0, time - MARQUEE_DWELL_SECS / 2),
            artistOverflow,
            applied.detailSpeed,
        ).toFixed(1)}px)`

        if (showTimes) {
            elapsedEl.textContent = formatTrackTime(data.media.positionMs())
            totalEl.textContent = formatTrackTime(media.durationMs)
        }

        const c = ctx.ctx
        c.clearRect(0, 0, ctx.width, ctx.height)
        if (atmosphereVisible(controls)) {
            drawNebulaField(c, ctx.width, ctx.height, time, accent, secondary, playing ? 1.0 : 0.55)
        }

        const progress = progressGlide.update(media.available ? data.media.progress() : 0, dt)
        const breathe = idlePulse.update(media.available ? 0 : 1, dt)

        if (controls.showProgress === true) {
            if (applied.ring) {
                arcGauge(c, {
                    cx: applied.ring.cx,
                    cy: applied.ring.cy,
                    fillColor: [accent, secondary],
                    glow: playing ? 0.55 : 0.18,
                    radius: applied.ring.radius,
                    startAngle: -Math.PI / 2,
                    sweep: Math.PI * 2,
                    thickness: applied.ring.thickness,
                    trackColor: withAlpha(ink.dim, 0.14),
                    value: media.available ? clamp01(progress) : 0,
                })
            } else {
                // The rail runs under the text column, which starts where
                // the layout put it.
                const bodyRect = marqueeEl.getBoundingClientRect()
                const railHeight = Math.max(3, ctx.height * 0.035)
                const railY = ctx.height - railHeight * 2.4
                const railLeft = bodyRect.left
                const railWidth = bodyRect.width
                c.fillStyle = withAlpha(ink.dim, 0.18)
                c.fillRect(railLeft, railY, railWidth, railHeight)
                if (media.available) {
                    const fillGradient = c.createLinearGradient(railLeft, 0, railLeft + railWidth, 0)
                    fillGradient.addColorStop(0, accent)
                    fillGradient.addColorStop(1, secondary)
                    c.fillStyle = fillGradient
                    c.fillRect(railLeft, railY, railWidth * clamp01(progress), railHeight)
                }
            }
        }

        if (!media.available && atmosphereVisible(controls) && applied.art > 0) {
            const pulse = 0.5 + 0.5 * Math.sin(time * 1.4)
            const glowRadius = applied.art * (0.62 + 0.05 * pulse) * breathe
            if (glowRadius > 1) {
                const artRect = artWrap.getBoundingClientRect()
                const cx = artRect.left + artRect.width / 2
                const cy = artRect.top + artRect.height / 2
                const gradient = c.createRadialGradient(cx, cy, 0, cx, cy, glowRadius)
                gradient.addColorStop(0, withAlpha(accent, 0.1 + 0.08 * pulse))
                gradient.addColorStop(1, withAlpha(accent, 0))
                c.fillStyle = gradient
                c.fillRect(0, 0, ctx.width, ctx.height)
            }
        }
    }
}
