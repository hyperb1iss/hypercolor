#!/usr/bin/env bun

import { chmodSync, cpSync, mkdirSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

const PACKAGE_ROOT = resolve(import.meta.dirname)
const DIST_DIR = join(PACKAGE_ROOT, 'dist')
const BIN_FILE = join(PACKAGE_ROOT, 'bin', 'hypercolor.js')
const TEMPLATES_DIR = join(PACKAGE_ROOT, 'templates')
const CREATE_EFFECT_TEMPLATES_DIR = join(PACKAGE_ROOT, '..', 'create-effect', 'templates')
const SHARED_PALETTES = join(PACKAGE_ROOT, '..', '..', 'shared', 'palettes.json')
const VENDORED_PALETTES = join(PACKAGE_ROOT, 'src', 'palette', 'palettes.gen.json')

async function buildOrThrow(config: Bun.BuildConfig): Promise<void> {
    const result = await Bun.build(config)
    if (!result.success) {
        throw new AggregateError(
            result.logs.map((log) => new Error(log.message)),
            `Bun.build failed for ${config.entrypoints?.join(', ') ?? config.outfile ?? 'build output'}`,
        )
    }
}

async function main(): Promise<void> {
    rmSync(DIST_DIR, { force: true, recursive: true })
    rmSync(TEMPLATES_DIR, { force: true, recursive: true })
    mkdirSync(DIST_DIR, { recursive: true })
    cpSync(CREATE_EFFECT_TEMPLATES_DIR, TEMPLATES_DIR, { recursive: true })
    // The npm tarball cannot reach sdk/shared, so vendor the palette data
    // inside the package. palettes-sync.test.ts asserts the copies match.
    cpSync(SHARED_PALETTES, VENDORED_PALETTES)

    await buildOrThrow({
        entrypoints: [join(PACKAGE_ROOT, 'src/index.ts')],
        format: 'esm',
        outdir: DIST_DIR,
        sourcemap: 'external',
        target: 'browser',
    })

    await buildOrThrow({
        entrypoints: [join(PACKAGE_ROOT, 'src/cli.ts'), join(PACKAGE_ROOT, 'src/tooling/metadata-worker.ts')],
        format: 'esm',
        naming: '[name].js',
        outdir: DIST_DIR,
        sourcemap: 'external',
        target: 'bun',
    })

    chmodSync(BIN_FILE, 0o755)
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
