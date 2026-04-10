import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const shader = readFileSync(resolve(import.meta.dirname, '../../../src/effects/breakthrough/fragment.glsl'), 'utf8')

describe('breakthrough shader motion stability', () => {
    test('keeps elapsed time out of the oscillating twist factor', () => {
        expect(shader).toMatch(/float twistBase = clamp\(iTwist \/ 100\.0, 0\.0, 1\.0\);/)
        expect(shader).toMatch(/float twist = twistBase \* \(0\.6 \+ 0\.4 \* sin\(time \* 0\.5\)\);/)
        expect(shader).toMatch(/float twistPhase = time \* twistBase \* 0\.4;/)
        expect(shader).not.toMatch(/time \* twist \* 0\.4/)
    })
})
