#!/usr/bin/env bun

import { chmodSync, mkdirSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

const PACKAGE_ROOT = resolve(import.meta.dirname)
const DIST_DIR = join(PACKAGE_ROOT, 'dist')
const BIN_FILE = join(PACKAGE_ROOT, 'bin', 'hypercolor.js')

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
    mkdirSync(DIST_DIR, { recursive: true })

    await buildOrThrow({
        entrypoints: [join(PACKAGE_ROOT, 'src/index.ts')],
        format: 'esm',
        outdir: DIST_DIR,
        sourcemap: 'external',
        target: 'browser',
    })

    await buildOrThrow({
        entrypoints: [join(PACKAGE_ROOT, 'src/cli.ts')],
        format: 'esm',
        outfile: join(DIST_DIR, 'cli.js'),
        sourcemap: 'external',
        target: 'bun',
    })

    await buildOrThrow({
        entrypoints: [join(PACKAGE_ROOT, 'src/tooling/metadata-worker.ts')],
        format: 'esm',
        outfile: join(DIST_DIR, 'tooling', 'metadata-worker.js'),
        sourcemap: 'external',
        target: 'bun',
    })

    chmodSync(BIN_FILE, 0o755)
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
