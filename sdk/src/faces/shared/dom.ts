import { withAlpha } from '@hypercolor/sdk'
import type { FaceContext } from '@hypercolor/sdk'

export const DISPLAY_FONT_FAMILIES = [
    'Orbitron',
    'Audiowide',
    'Bebas Neue',
    'Exo 2',
    'Rajdhani',
    'Space Mono',
    'JetBrains Mono',
] as const

export const UI_FONT_FAMILIES = [
    'Sora',
    'Space Grotesk',
    'DM Sans',
    'Inter',
    'Roboto Condensed',
    'Exo 2',
] as const

export function ensureFaceStyles(id: string, css: string): void {
    if (document.getElementById(id)) return
    const style = document.createElement('style')
    style.id = id
    style.textContent = css
    document.head.appendChild(style)
}

export function createFaceRoot(ctx: FaceContext, className: string): HTMLDivElement {
    const existing = ctx.container.querySelector<HTMLDivElement>(`:scope > .${className}`)
    if (existing) {
        existing.innerHTML = ''
        existing.style.background = 'transparent'
        return existing
    }

    const root = document.createElement('div')
    root.className = className
    root.style.position = 'absolute'
    root.style.inset = '0'
    root.style.zIndex = '3'
    root.style.pointerEvents = 'none'
    root.style.display = 'grid'
    root.style.width = '100%'
    root.style.height = '100%'
    root.style.background = 'transparent'
    root.style.placeItems = 'stretch'
    ctx.container.appendChild(root)
    return root
}

export function humanizeSensorLabel(label: string): string {
    if (!label) return 'Unassigned'
    return label
        .replace(/[_-]+/g, ' ')
        .replace(/\b\w/g, (match) => match.toUpperCase())
}

export function clamp01(value: number): number {
    return Math.max(0, Math.min(1, value))
}

function faceBackdropStrength(backdrop: string, opaque: number, glass: number, clear: number): number {
    switch (backdrop.toLowerCase()) {
        case 'opaque':
            return opaque
        case 'glass':
            return glass
        case 'clear':
        default:
            return clear
    }
}

export function resolveFaceSurface(
    backdrop: string,
    color: string,
    alphaPercent: number,
    strengths: { opaque?: number; glass?: number; clear?: number } = {},
): string {
    const alpha = clamp01(alphaPercent / 100)
    const strength = faceBackdropStrength(
        backdrop,
        strengths.opaque ?? 1,
        strengths.glass ?? 0.56,
        strengths.clear ?? 0.16,
    )
    return withAlpha(color, alpha * strength)
}

export function resolveFaceCanvasWash(
    backdrop: string,
    color: string,
    alphaPercent: number,
    strengths: { opaque?: number; glass?: number; clear?: number } = {},
): string | null {
    const alpha = clamp01(alphaPercent / 100)
    const strength = faceBackdropStrength(
        backdrop,
        strengths.opaque ?? 1,
        strengths.glass ?? 0.32,
        strengths.clear ?? 0,
    )
    const finalAlpha = alpha * strength
    return finalAlpha > 0 ? withAlpha(color, finalAlpha) : null
}
