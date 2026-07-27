#!/usr/bin/env bun
/**
 * Cover artwork sync tool.
 *
 * Installs `cover.webp` beside each effect/face `main.ts` so the build can embed
 * it inline. Source art is taken from capture drafts when present, otherwise
 * from the docs site imagery, and is downscaled to a card-sized WebP.
 *
 * Covers ride inline in every built artifact, so the output is deliberately
 * small — full-resolution art stays in docs/static/img/effects/.
 */

import { existsSync } from 'node:fs'
import { mkdir, readdir, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'

import sharp from 'sharp'

import { artifactIdFromEntry, extractArtifactMetadata } from '../packages/core/src/tooling'

const SDK_ROOT = resolve(import.meta.dirname, '..')
const REPO_ROOT = resolve(SDK_ROOT, '..')
const ENTRY_ROOTS = ['src/effects', 'src/faces']
const DRAFTS_ROOT = resolve(REPO_ROOT, 'effects', 'screenshots', 'drafts')
const DOCS_ART_ROOT = resolve(REPO_ROOT, 'docs', 'static', 'img', 'effects')

/**
 * Cards render around 300-400 CSS px and zoom to 1.06 on hover, so a 150%-scale
 * display asks for roughly 600 device pixels. 960 leaves headroom above that
 * without doubling what every artifact carries inline.
 *
 * Daemon captures top out at the 640x480 compositor canvas; `withoutEnlargement`
 * keeps those at native rather than upscaling them into a bigger, blurrier file.
 */
const COVER_WIDTH = 960
const COVER_QUALITY = 88

interface CliOptions {
    effectFilter: string | null
    force: boolean
    dryRun: boolean
}

function parseArgs(argv: string[]): CliOptions {
    const options: CliOptions = { dryRun: false, effectFilter: null, force: false }

    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index]
        if (arg === '--effect') {
            options.effectFilter = argv[index + 1] ?? null
            index += 1
        } else if (arg === '--force') {
            options.force = true
        } else if (arg === '--dry-run') {
            options.dryRun = true
        }
    }

    return options
}

function slugify(value: string): string {
    return value
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-|-$/g, '')
}

async function discoverEntries(): Promise<string[]> {
    const entries: string[] = []

    for (const root of ENTRY_ROOTS) {
        const absolute = resolve(SDK_ROOT, root)
        if (!existsSync(absolute)) continue

        for (const dirent of await readdir(absolute, { withFileTypes: true })) {
            if (!dirent.isDirectory() || dirent.name.startsWith('_')) continue
            const mainFile = join(absolute, dirent.name, 'main.ts')
            if (existsSync(mainFile)) entries.push(mainFile)
        }
    }

    return entries.sort()
}

/** Freshly captured drafts win over docs art so a re-capture is what ships. */
function resolveSourceArt(slug: string): { path: string; origin: string } | null {
    const draft = join(DRAFTS_ROOT, slug, 'default', 'rank-1.png')
    if (existsSync(draft)) return { origin: 'draft', path: draft }

    const docs = join(DOCS_ART_ROOT, `${slug}.webp`)
    if (existsSync(docs)) return { origin: 'docs', path: docs }

    return null
}

async function main(): Promise<void> {
    const options = parseArgs(Bun.argv.slice(2))
    const entries = await discoverEntries()

    let installed = 0
    let skipped = 0
    const missing: string[] = []

    for (const entryPath of entries) {
        const id = artifactIdFromEntry(entryPath)
        if (options.effectFilter && id !== options.effectFilter) continue

        const metadata = await extractArtifactMetadata(entryPath)
        const slug = slugify(metadata.name)
        const coverPath = join(dirname(entryPath), 'cover.webp')

        if (existsSync(coverPath) && !options.force) {
            skipped += 1
            continue
        }

        const source = resolveSourceArt(slug)
        if (!source) {
            missing.push(`${id} (${metadata.name} -> ${slug})`)
            continue
        }

        const encoded = await sharp(source.path)
            .resize(COVER_WIDTH, null, { withoutEnlargement: true })
            .webp({ quality: COVER_QUALITY })
            .toBuffer()

        if (!options.dryRun) {
            await mkdir(dirname(coverPath), { recursive: true })
            await writeFile(coverPath, encoded)
        }

        installed += 1
        console.log(`  ✓ ${id.padEnd(24)} ${(encoded.length / 1024).toFixed(0).padStart(4)}KB  from ${source.origin}`)
    }

    console.log(`\n💎 ${installed} cover${installed === 1 ? '' : 's'} installed, ${skipped} already present`)

    if (missing.length > 0) {
        console.log(`\n⚠️  no source art for ${missing.length}:`)
        for (const entry of missing) console.log(`     ${entry}`)
    }

    if (options.dryRun) console.log('\n(dry run — nothing written)')
}

await main()
