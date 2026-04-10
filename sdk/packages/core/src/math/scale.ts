/**
 * Resolution-independent scaling for effects.
 *
 * The Hypercolor daemon renders at whatever canvas size the user has configured
 * (640x480 by default). Effects must never hardcode dimensions. This module
 * provides a [`ScaleContext`] that bridges a "design basis" (the coordinate
 * system the author is comfortable drawing in) to the live canvas size.
 *
 * ## Usage: declarative effects
 *
 * ```ts
 * import { canvas, scaleContext } from '@hypercolor/sdk'
 *
 * canvas('Bubble Garden', controls, (ctx, time, controls) => {
 *     const s = scaleContext(ctx.canvas, { width: 320, height: 200 })
 *     ctx.beginPath()
 *     ctx.arc(s.dx(160), s.dy(100), s.ds(14), 0, Math.PI * 2)
 *     ctx.fill()
 * })
 * ```
 *
 * ## Usage: class-based effects
 *
 * ```ts
 * class Aurora extends CanvasEffect<Controls> {
 *     protected designBasis = { width: 320, height: 200 }
 *
 *     protected draw(time: number) {
 *         const s = this.scaleContext()
 *         this.ctx!.fillRect(s.dx(10), s.dy(20), s.dw(50), s.dh(30))
 *     }
 * }
 * ```
 *
 * ## Pure-adaptive effects
 *
 * Effects that don't need a fixed design basis can skip the helper entirely
 * and read `ctx.canvas.width` / `ctx.canvas.height` directly. Or pass no
 * `designBasis` to [`scaleContext`] — the scale factors all become 1 and
 * `dx/dy/dw/dh` become identity functions.
 */

/** Coordinate system an effect is authored against. */
export interface DesignBasis {
    /** Design-space width (e.g. 320). */
    width: number
    /** Design-space height (e.g. 200). */
    height: number
}

/**
 * Resolution-independent coordinate helpers bound to a live canvas size and
 * an optional design basis. Cheap to construct — build one per frame.
 */
export interface ScaleContext {
    /** Live canvas width in pixels. */
    readonly width: number
    /** Live canvas height in pixels. */
    readonly height: number
    /** Horizontal scale factor: `width / designBasis.width`. */
    readonly sx: number
    /** Vertical scale factor: `height / designBasis.height`. */
    readonly sy: number
    /** Uniform scale: `min(sx, sy)`. Preserves aspect ratio. */
    readonly scale: number
    /** Convert a design-space X coordinate to live pixels. */
    dx(x: number): number
    /** Convert a design-space Y coordinate to live pixels. */
    dy(y: number): number
    /** Convert a design-space width to live pixels. */
    dw(w: number): number
    /** Convert a design-space height to live pixels. */
    dh(h: number): number
    /** Uniform-scale a value (radii, stroke widths, font sizes). */
    ds(value: number): number
    /** Normalized `[0,1]` → live X pixels. */
    nx(t: number): number
    /** Normalized `[0,1]` → live Y pixels. */
    ny(t: number): number
}

/** Minimal shape [`scaleContext`] accepts — anything with integer dimensions. */
export interface CanvasSize {
    readonly width: number
    readonly height: number
}

/**
 * Build a [`ScaleContext`] from a canvas (or any `{width, height}` object) and
 * an optional design basis. If `designBasis` is omitted, the scale is the
 * identity (`sx = sy = 1`) and design/normalized helpers echo their inputs.
 *
 * Call this fresh inside your draw function — it's just a few arithmetic ops,
 * and snapshotting per frame keeps you consistent if the canvas resizes mid-run.
 */
export function scaleContext(source: CanvasSize, designBasis?: DesignBasis): ScaleContext {
    const width = source.width
    const height = source.height
    const basisWidth = designBasis?.width ?? width
    const basisHeight = designBasis?.height ?? height
    const sx = basisWidth > 0 ? width / basisWidth : 1
    const sy = basisHeight > 0 ? height / basisHeight : 1
    const scale = Math.min(sx, sy)

    return {
        dh: (h) => h * sy,
        ds: (v) => v * scale,
        dw: (w) => w * sx,
        dx: (x) => x * sx,
        dy: (y) => y * sy,
        height,
        nx: (t) => t * width,
        ny: (t) => t * height,
        scale,
        sx,
        sy,
        width,
    }
}
