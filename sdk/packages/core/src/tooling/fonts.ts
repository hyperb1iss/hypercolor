/**
 * Build-time font embedding for display faces.
 *
 * Servo renders faces in capture mode, which skips the runtime Google
 * Fonts loader — so built faces must carry their own type. The build
 * resolves every font-picker control to the vendored latin woff2 files
 * (sdk/assets/fonts, see scripts/fetch-fonts.ts) and inlines them as
 * @font-face data URLs, keeping faces single-file and network-free.
 */

import { existsSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'

import type { BuildControlDef } from './types'

/** Weights embedded for a font control that doesn't declare its own. */
const DEFAULT_EMBED_WEIGHTS = [400, 600]

interface FontManifest {
    families: Record<
        string,
        {
            family: string
            license: string
            licenseFile: string
            weights: Record<string, string>
        }
    >
}

function isFontControl(control: BuildControlDef): boolean {
    if (control.type !== 'combobox' || !control.values?.length) return false
    const id = control.id.toLowerCase()
    const label = (control.label ?? '').toLowerCase()
    return id.includes('font') || label.includes('font')
}

/** family → requested weights, unioned across the face's font controls. */
export function resolveFaceFontEmbedPlan(controls: BuildControlDef[]): Map<string, Set<number>> {
    const plan = new Map<string, Set<number>>()
    for (const control of controls) {
        if (!isFontControl(control)) continue
        const weights = control.fontWeights?.length ? control.fontWeights : DEFAULT_EMBED_WEIGHTS
        for (const family of control.values ?? []) {
            const entry = plan.get(family) ?? new Set<number>()
            for (const weight of weights) entry.add(weight)
            plan.set(family, entry)
        }
    }
    return plan
}

/** Locate sdk/assets/fonts from either src or dist layouts. */
export function defaultFontAssetsDir(): string | undefined {
    let dir = import.meta.dirname
    for (let depth = 0; depth < 6; depth += 1) {
        const candidate = join(dir, 'assets', 'fonts')
        if (existsSync(join(candidate, 'manifest.json'))) return candidate
        const parent = dirname(dir)
        if (parent === dir) break
        dir = parent
    }
    return undefined
}

function nearestWeight(available: number[], wanted: number): number | undefined {
    let best: number | undefined
    let bestDistance = Number.POSITIVE_INFINITY
    for (const candidate of available) {
        const distance = Math.abs(candidate - wanted)
        if (distance < bestDistance || (distance === bestDistance && candidate < (best ?? 0))) {
            bestDistance = distance
            best = candidate
        }
    }
    return best
}

/**
 * Emit @font-face CSS for the embed plan from the vendored assets.
 * Families or weights missing from the manifest resolve to their nearest
 * vendored weight (or are skipped with a warning) so builds never fail
 * on typography.
 */
export function buildFontFaceCss(plan: Map<string, Set<number>>, fontAssetsDir?: string): string {
    if (plan.size === 0) return ''
    const assetsDir = fontAssetsDir ?? defaultFontAssetsDir()
    if (!assetsDir) {
        console.warn('font embed: sdk/assets/fonts not found — faces will fall back to system fonts')
        return ''
    }

    const manifest = JSON.parse(readFileSync(join(assetsDir, 'manifest.json'), 'utf8')) as FontManifest
    const rules: string[] = []

    for (const [family, wantedWeights] of [...plan.entries()].sort(([a], [b]) => a.localeCompare(b))) {
        const entry = manifest.families[family]
        if (!entry) {
            console.warn(`font embed: '${family}' is not vendored — it will fall back to system fonts`)
            continue
        }
        const available = Object.keys(entry.weights).map(Number)
        const embedWeights = new Set<number>()
        for (const wanted of [...wantedWeights].sort((a, b) => a - b)) {
            const nearest = nearestWeight(available, wanted)
            if (nearest != null) embedWeights.add(nearest)
        }
        for (const weight of [...embedWeights].sort((a, b) => a - b)) {
            const relativePath = entry.weights[String(weight)]
            if (!relativePath) continue
            const filePath = resolve(assetsDir, relativePath)
            if (!existsSync(filePath)) {
                console.warn(`font embed: missing font file ${filePath}`)
                continue
            }
            const base64 = readFileSync(filePath).toString('base64')
            rules.push(
                `@font-face {\n  font-family: '${family}';\n  font-style: normal;\n  font-weight: ${weight};\n  src: url(data:font/woff2;base64,${base64}) format('woff2');\n}`,
            )
        }
    }

    return rules.join('\n')
}

/** Convenience: full embed pipeline for a face's build controls. */
export function faceFontFaceCss(controls: BuildControlDef[], fontAssetsDir?: string): string {
    return buildFontFaceCss(resolveFaceFontEmbedPlan(controls), fontAssetsDir)
}
