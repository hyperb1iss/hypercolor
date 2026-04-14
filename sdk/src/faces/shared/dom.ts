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
