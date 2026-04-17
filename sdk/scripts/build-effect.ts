#!/usr/bin/env bun
import { resolve } from 'node:path'

import { buildArtifacts, discoverWorkspaceEntries } from '../packages/core/src/tooling'

const SDK_ROOT = resolve(import.meta.dirname, '..')
const DEFAULT_OUT = resolve(SDK_ROOT, '..', 'effects', 'hypercolor')
const SDK_ALIAS = resolve(SDK_ROOT, 'packages/core/src/index.ts')

function parseArgs() {
    const args = process.argv.slice(2)
    let outDir = DEFAULT_OUT
    let buildAll = false
    let buildFacesOnly = false
    const entries: string[] = []

    for (let index = 0; index < args.length; index += 1) {
        const arg = args[index]
        const nextArg = args[index + 1]
        if (arg === '--out' && nextArg) {
            outDir = resolve(nextArg)
            index += 1
            continue
        }
        if (arg === '--all') {
            buildAll = true
            continue
        }
        if (arg === '--faces') {
            buildFacesOnly = true
            continue
        }
        entries.push(resolve(arg))
    }

    if (buildAll || buildFacesOnly) {
        const roots = buildFacesOnly ? ['src/faces'] : ['src/effects', 'src/faces']
        entries.push(...discoverWorkspaceEntries(SDK_ROOT, roots))
    }

    if (entries.length === 0) {
        throw new Error('Usage: bun scripts/build-effect.ts [--all | --faces | <entry.ts>...]')
    }

    return { entries, outDir }
}

async function main() {
    const { entries, outDir } = parseArgs()

    console.log('\x1b[38;2;225;53;255m  Hypercolor Effect Builder\x1b[0m')
    console.log(`  Output: ${outDir}\n`)

    const results = await buildArtifacts({
        entryPaths: entries,
        outDir,
        sdkAliasPath: SDK_ALIAS,
    })

    for (const result of results) {
        const label = result.kind === 'face' ? 'face' : 'effect'
        const icon = result.kind === 'face' ? '💎' : '✓'
        const sizeKB = (result.bytes / 1024).toFixed(1)
        console.log(`\x1b[38;2;128;255;234m  Building ${label}\x1b[0m ${result.id}`)
        console.log(`\x1b[38;2;80;250;123m  ${icon}\x1b[0m ${result.outputPath} (${sizeKB} KB)`)
    }

    const faceCount = results.filter((result) => result.kind === 'face').length
    const effectCount = results.length - faceCount
    const parts = []
    if (effectCount > 0) parts.push(`${effectCount} effect(s)`)
    if (faceCount > 0) parts.push(`${faceCount} face(s)`)
    console.log(`\n\x1b[38;2;80;250;123m  ✓ ${parts.join(' + ')} built\x1b[0m`)
}

main().catch((error) => {
    console.error('\x1b[38;2;255;99;99m  ✗ Build failed:\x1b[0m', error)
    process.exit(1)
})
