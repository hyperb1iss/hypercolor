import type { FaceContext } from 'hypercolor'

/**
 * The widest centered line that stays on the panel between two heights.
 *
 * A round panel's inscribed square is the safe rectangle for a whole
 * layout, but a single line near the middle has far more room than that:
 * the panel is as wide as its chord at the line's far edge. Other shapes
 * give every line the safe width.
 */
export function clearWidth(ctx: FaceContext, top: number, bottom: number, inset: number): number {
    if (!ctx.display.circular) {
        return Math.max(0, ctx.display.safeArea.width - 2 * inset)
    }
    const radius = Math.min(ctx.width, ctx.height) / 2 - inset
    const centerY = ctx.height / 2
    const reach = Math.max(Math.abs(top - centerY), Math.abs(bottom - centerY))
    if (reach >= radius) return 0
    return 2 * Math.sqrt(radius * radius - reach * reach)
}
