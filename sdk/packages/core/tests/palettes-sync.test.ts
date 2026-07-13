import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const SHARED = resolve(import.meta.dirname, '../../../shared/palettes.json')
const VENDORED = resolve(import.meta.dirname, '../src/palette/palettes.gen.json')

describe('vendored palette data', () => {
    test('palettes.gen.json is byte-identical to sdk/shared/palettes.json', () => {
        // sdk/shared/palettes.json is the cross-language source of truth (the
        // Rust engine embeds it via include_str!). The package vendors a copy
        // because the npm tarball cannot reach outside the package root.
        // Re-sync with `bun run build` in packages/core if this fails.
        expect(readFileSync(VENDORED, 'utf8')).toBe(readFileSync(SHARED, 'utf8'))
    })
})
