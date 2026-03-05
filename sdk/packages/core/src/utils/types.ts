/** The update function signature used by effects. */
export type UpdateFunction = (force?: boolean) => void

/** RGB color as individual channels (0-1). */
export interface RGBColor {
    r: number
    g: number
    b: number
}

/** HSL color representation. */
export interface HSLColor {
    h: number
    s: number
    l: number
}
