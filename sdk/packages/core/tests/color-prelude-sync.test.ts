import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { COLOR_PRELUDE } from '../src/tooling/color-prelude.gen'

const SHARED = resolve(import.meta.dirname, '../../../shared/glsl/color.glsl')

describe('vendored GLSL color prelude', () => {
    test('color-prelude.gen.ts is byte-identical to sdk/shared/glsl/color.glsl', () => {
        // sdk/shared/glsl/color.glsl is the source of truth for the shader-side
        // color helpers. The package vendors a copy because the npm tarball
        // cannot reach outside the package root. Re-sync with `bun run build`
        // in packages/core if this fails.
        expect(COLOR_PRELUDE).toBe(readFileSync(SHARED, 'utf8'))
    })
})
