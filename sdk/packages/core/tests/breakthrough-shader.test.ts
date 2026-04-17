import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const shader = readFileSync(resolve(import.meta.dirname, '../../../src/effects/breakthrough/fragment.glsl'), 'utf8')
const main = readFileSync(resolve(import.meta.dirname, '../../../src/effects/breakthrough/main.ts'), 'utf8')

describe('breakthrough shader motion stability', () => {
    test('keeps elapsed time out of the oscillating twist factor', () => {
        expect(shader).toMatch(/float twistBase = clamp\(iTwist \/ 100\.0, 0\.0, 1\.0\);/)
        expect(shader).toMatch(/float twist = twistBase \* \(0\.6 \+ 0\.4 \* sin\(time \* 0\.5\)\);/)
        expect(shader).toMatch(/float twistPhase = time \* twistBase \* 0\.4;/)
        expect(shader).not.toMatch(/time \* twist \* 0\.4/)
    })

    test('adds a fractal style path and layered kaleidoscope structure', () => {
        expect(main).not.toContain("intensity: num('Intensity'")
        expect(main).toContain("'Fractal'")
        expect(shader).toContain(
            'float nestedSegments = max(3.0, segments + 2.0 + floor(pulseControl * 2.0 + warp * 2.0));',
        )
        expect(shader).toContain(
            'float blossom = sin((waveAngular + waveNested) * 2.4 + radius * (7.0 + segments) - time * (2.0 + pulseControl * 1.4));',
        )
        expect(shader).toContain('float depth = -log(radius + 0.06);')
        expect(shader).toContain(
            'uv += radialDir * flowControl * (shell * (0.018 + 0.028 * pulseControl) + shellAlt * 0.012);',
        )
        expect(shader).not.toContain('uniform float iColorIntensity;')
        expect(shader).toContain('vec3 liftMids(vec3 color, float amount) {')
        expect(shader).toContain('// Fractal — recursive petal overlays and a secondary psychedelic bloom')
        expect(shader).toContain('color = limitWhiteness(color, 0.32);')
    })
})
