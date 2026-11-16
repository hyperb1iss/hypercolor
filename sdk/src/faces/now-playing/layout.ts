import type { FaceContext } from 'hypercolor'
import { clearWidth } from '../shared/typography'

/** Line box heights as multiples of the font size. */
const TITLE_LINE = 1.16
const DETAIL_LINE = 1.3
/** The times row runs a touch under the artist line. */
const TIMES_RATIO = 0.9
/** Art smaller than this (at the 480 basis) reads as clutter; text takes the room. */
const MIN_ART = 64
/** The inscribed square of a 480 round panel, the basis sizes are quoted on. */
const ROUND_BASIS = 339

export interface NowPlayingSizes {
    /** Title size in pixels on a 480 panel. */
    titleSize: number
    /** Artist and times size in pixels on a 480 panel. */
    detailSize: number
    /** Art diameter as a percent of the panel's safe area. */
    artSize: number
    showArt: boolean
    showTimes: boolean
    showProgress: boolean
}

export interface ProgressRing {
    cx: number
    cy: number
    radius: number
    thickness: number
}

/** One centered line of text: where it sits and how wide it may run. */
export interface TextLine {
    fontSize: number
    height: number
    width: number
}

export interface StackLayout {
    /** Art diameter; zero when the text needs the room or art is off. */
    art: number
    /** Space the progress ring takes on each side of the art. */
    artOutset: number
    gap: number
    title: TextLine
    artist: TextLine
    times: TextLine
    ring: ProgressRing
}

export interface StripLayout {
    art: number
    gap: number
    titleSize: number
    artistSize: number
    timesSize: number
}

function ringOutset(art: number): number {
    return Math.max(8, art * 0.07) + ringThickness(art) / 2
}

function ringThickness(art: number): number {
    return Math.max(3.5, art * 0.035)
}

/**
 * Lay out the centered stack: art over title, artist, and times.
 *
 * Text is sized first and the art takes what is left, so a larger type
 * setting shrinks the art instead of pushing a line off the panel. When
 * the art would get too small to read it steps aside and the progress
 * ring moves out to the panel edge. Each line then runs as wide as the
 * panel allows at its own height, which on a round panel is well past
 * the inscribed square near the middle.
 */
export function layoutStack(ctx: FaceContext, sizes: NowPlayingSizes): StackLayout {
    const safe = ctx.display.safeArea
    const short = Math.min(safe.width, safe.height)
    const basis = short / ROUND_BASIS
    const gap = 10 * basis
    const gaps = gap * (sizes.showTimes ? 2 : 1)
    // The stack centers on the panel, so it has the safe room that lies
    // evenly either side of that center.
    const centerX = ctx.width / 2
    const centerY = ctx.height / 2
    const room = 2 * Math.min(centerY - safe.y, safe.y + safe.height - centerY)

    let titleSize = sizes.titleSize * basis
    let detailSize = sizes.detailSize * basis
    const linesHeight = (title: number, detail: number): number =>
        title * TITLE_LINE + detail * DETAIL_LINE + (sizes.showTimes ? detail * TIMES_RATIO * DETAIL_LINE : 0)
    const textHeight = (title: number, detail: number): number => linesHeight(title, detail) + gaps

    let art = 0
    if (sizes.showArt) {
        const wanted = (sizes.artSize / 100) * short
        const left = room - textHeight(titleSize, detailSize) - gap
        // The ring's outset grows with the art, so solve for the art that
        // fills what is left with its ring on.
        const ringShare = sizes.showProgress ? 1.175 : 1
        const ringFloor = sizes.showProgress ? 2 * ringOutset(0) : 0
        const fitted = Math.min(wanted, left / ringShare, left - ringFloor)
        art = fitted >= MIN_ART * basis ? fitted : 0
    }
    if (art === 0) {
        const fit = Math.min(1, (room - gaps) / linesHeight(titleSize, detailSize))
        titleSize *= fit
        detailSize *= fit
    }

    const artOutset = art > 0 && sizes.showProgress ? ringOutset(art) : 0
    const artBlock = art > 0 ? art + 2 * artOutset + gap : 0
    const total = artBlock + textHeight(titleSize, detailSize)
    // Clear of the bezel, which crops the outermost pixels on real panels.
    const inset = 22 * basis

    let cursor = centerY - total / 2 + artBlock
    const line = (fontSize: number, lineHeight: number): TextLine => {
        const height = fontSize * lineHeight
        const width = clearWidth(ctx, cursor, cursor + height, inset)
        cursor += height + gap
        return { fontSize, height, width }
    }
    const title = line(titleSize, TITLE_LINE)
    const artist = line(detailSize, DETAIL_LINE)
    const times = line(detailSize * TIMES_RATIO, DETAIL_LINE)

    const edgeThickness = Math.max(4, Math.min(ctx.width, ctx.height) * 0.014)
    const ring: ProgressRing =
        art > 0
            ? {
                  cx: centerX,
                  cy: centerY - total / 2 + artOutset + art / 2,
                  radius: art / 2 + Math.max(8, art * 0.07),
                  thickness: ringThickness(art),
              }
            : {
                  cx: ctx.width / 2,
                  cy: ctx.height / 2,
                  radius: Math.min(ctx.width, ctx.height) / 2 - edgeThickness / 2 - 3,
                  thickness: edgeThickness,
              }

    return { art, artist, artOutset, gap, ring, times, title }
}

/** Lay out the wide strip: art on the left, text beside it, rail below. */
export function layoutStrip(ctx: FaceContext, sizes: NowPlayingSizes): StripLayout {
    // A 160-tall strip shows a 44 title at about 30 pixels.
    const basis = (ctx.height / 480) * 2.05
    const gap = Math.max(3, ctx.height * 0.04)
    const gaps = gap * (sizes.showTimes ? 2 : 1)
    const title = sizes.titleSize * basis
    const detail = sizes.detailSize * basis
    const times = detail * 0.85
    const lines = title * TITLE_LINE + detail * DETAIL_LINE + (sizes.showTimes ? times * DETAIL_LINE : 0)
    const room = ctx.height * (sizes.showProgress ? 0.76 : 0.9)
    const fit = Math.min(1, (room - gaps) / lines)
    const artShare = Math.min(0.9, Math.max(0.4, (0.78 * sizes.artSize) / 44))
    return {
        art: sizes.showArt ? ctx.height * artShare : 0,
        artistSize: detail * fit,
        gap,
        timesSize: times * fit,
        titleSize: title * fit,
    }
}
