import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const main = readFileSync(resolve(import.meta.dirname, '../../../src/effects/deep-current/main.ts'), 'utf8')

describe('deep current palette contract', () => {
    test('declares an explicit palette control for the numeric frame hook path', () => {
        expect(main).toContain("palette: paletteControl('Palette', PALETTE_NAMES, {")
        expect(main).toContain('const idx = ctx.controls.palette as number')
    })
})
