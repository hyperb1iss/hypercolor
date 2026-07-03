#!/usr/bin/env bun
/**
 * Vendor the curated face fonts into sdk/assets/fonts/.
 *
 * Downloads latin-subset woff2 files (per family and weight) from the
 * Google Fonts CSS API plus each family's upstream license text, and
 * writes manifest.json describing what was vendored. The outputs are
 * checked in so face builds are hermetic — no network at build time.
 *
 * Re-run only when the curated family set changes: bun scripts/fetch-fonts.ts
 */

import { mkdirSync } from 'node:fs'
import { join, resolve } from 'node:path'

const OUT_ROOT = resolve(import.meta.dirname, '..', 'assets', 'fonts')

/** A browser UA so the CSS API serves woff2 sources. */
const UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36'

interface FamilySpec {
    family: string
    /** Weights faces actually use; single-weight families list what exists. */
    weights: number[]
    /** Directory name in the google/fonts repo (for license retrieval). */
    gfDir: string
    license: 'OFL-1.1'
}

const FAMILIES: FamilySpec[] = [
    { family: 'Audiowide', gfDir: 'ofl/audiowide', license: 'OFL-1.1', weights: [400] },
    { family: 'Bebas Neue', gfDir: 'ofl/bebasneue', license: 'OFL-1.1', weights: [400] },
    { family: 'DM Sans', gfDir: 'ofl/dmsans', license: 'OFL-1.1', weights: [300, 400, 500, 600] },
    { family: 'Exo 2', gfDir: 'ofl/exo2', license: 'OFL-1.1', weights: [300, 400, 500, 600] },
    { family: 'Inter', gfDir: 'ofl/inter', license: 'OFL-1.1', weights: [300, 400, 500, 600] },
    { family: 'JetBrains Mono', gfDir: 'ofl/jetbrainsmono', license: 'OFL-1.1', weights: [300, 400, 500, 600] },
    { family: 'Orbitron', gfDir: 'ofl/orbitron', license: 'OFL-1.1', weights: [400, 500, 600] },
    { family: 'Rajdhani', gfDir: 'ofl/rajdhani', license: 'OFL-1.1', weights: [300, 400, 500, 600] },
    { family: 'Roboto Condensed', gfDir: 'ofl/robotocondensed', license: 'OFL-1.1', weights: [300, 400, 500, 600] },
    { family: 'Sora', gfDir: 'ofl/sora', license: 'OFL-1.1', weights: [300, 400, 500, 600] },
    { family: 'Space Grotesk', gfDir: 'ofl/spacegrotesk', license: 'OFL-1.1', weights: [300, 400, 500, 600] },
    { family: 'Space Mono', gfDir: 'ofl/spacemono', license: 'OFL-1.1', weights: [400, 700] },
]

function slugify(family: string): string {
    return family.toLowerCase().replaceAll(/\s+/g, '-')
}

interface LatinFace {
    weight: number
    url: string
}

/** Pull the latin-subset woff2 URL for each weight out of css2 output. */
function parseLatinFaces(css: string): LatinFace[] {
    const faces: LatinFace[] = []
    const blocks = css.split('@font-face')
    for (const block of blocks) {
        // The latin subset covers U+0000-00FF; ignore latin-ext and others.
        if (!block.includes('U+0000-00FF')) continue
        const weight = block.match(/font-weight:\s*(\d+)/)?.[1]
        const url = block.match(/src:\s*url\((https:[^)]+\.woff2)\)/)?.[1]
        if (weight && url) faces.push({ url, weight: Number(weight) })
    }
    return faces
}

interface ManifestEntry {
    family: string
    license: string
    licenseFile: string
    weights: Record<string, string>
}

async function fetchFamily(spec: FamilySpec): Promise<ManifestEntry> {
    const slug = slugify(spec.family)
    const dir = join(OUT_ROOT, slug)
    mkdirSync(dir, { recursive: true })

    const axis = spec.weights.join(';')
    const cssUrl = `https://fonts.googleapis.com/css2?family=${encodeURIComponent(spec.family).replaceAll('%20', '+')}:wght@${axis}&display=swap`
    const cssResponse = await fetch(cssUrl, { headers: { 'user-agent': UA } })
    if (!cssResponse.ok) {
        throw new Error(`css2 request failed for ${spec.family}: HTTP ${cssResponse.status}`)
    }
    const faces = parseLatinFaces(await cssResponse.text())

    const weights: Record<string, string> = {}
    for (const weight of spec.weights) {
        const match = faces.find((face) => face.weight === weight)
        if (!match) {
            throw new Error(`no latin woff2 for ${spec.family} weight ${weight}`)
        }
        const fileName = `${slug}-${weight}.woff2`
        const fontResponse = await fetch(match.url, { headers: { 'user-agent': UA } })
        if (!fontResponse.ok) {
            throw new Error(`font download failed for ${spec.family} ${weight}: HTTP ${fontResponse.status}`)
        }
        const bytes = new Uint8Array(await fontResponse.arrayBuffer())
        await Bun.write(join(dir, fileName), bytes)
        weights[String(weight)] = `${slug}/${fileName}`
        console.log(`  ${spec.family} ${weight} → ${fileName} (${(bytes.length / 1024).toFixed(1)} KB)`)
    }

    const licenseName = spec.license === 'OFL-1.1' ? 'OFL.txt' : 'LICENSE.txt'
    const licenseUrl = `https://raw.githubusercontent.com/google/fonts/main/${spec.gfDir}/${licenseName}`
    const licenseResponse = await fetch(licenseUrl, { headers: { 'user-agent': UA } })
    if (!licenseResponse.ok) {
        throw new Error(`license download failed for ${spec.family}: HTTP ${licenseResponse.status} (${licenseUrl})`)
    }
    const licenseFile = `${slug}/${licenseName}`
    await Bun.write(join(OUT_ROOT, licenseFile), await licenseResponse.text())

    return { family: spec.family, license: spec.license, licenseFile, weights }
}

console.log(`Vendoring ${FAMILIES.length} families into ${OUT_ROOT}\n`)
const entries: ManifestEntry[] = []
for (const spec of FAMILIES) {
    entries.push(await fetchFamily(spec))
}

const manifest = {
    families: Object.fromEntries(entries.map((entry) => [entry.family, entry])),
    generatedBy: 'sdk/scripts/fetch-fonts.ts',
    subset: 'latin',
}
await Bun.write(join(OUT_ROOT, 'manifest.json'), `${JSON.stringify(manifest, null, 4)}\n`)
console.log(`\nmanifest.json written — ${entries.length} families vendored`)
