import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const main = readFileSync(resolve(import.meta.dirname, '../../../src/effects/iris/main.ts'), 'utf8')
const shader = readFileSync(resolve(import.meta.dirname, '../../../src/effects/iris/fragment.glsl'), 'utf8')

describe('iris effect regressions', () => {
    test('keeps user-facing controls descriptive', () => {
        expect(main).toContain("glowIntensity: num('Halo'")
        expect(main).toContain("colorAccent: num('Color Split'")
        expect(main).toContain("scale: num('Zoom'")
        expect(main).toContain("irisStrength: num('Ripple Density'")
        expect(main).toContain("corePulse: num('Center Beam'")
        expect(main).toContain("bandSharpness: num('Band Sharpness'")
        expect(main).toContain("particleDensity: num('Particle Fabric'")
    })

    test('exposes the expanded palette set and curated presets', () => {
        expect(main).toContain("'Abyss Bloom'")
        expect(main).toContain("'Circuit Jade'")
        expect(main).toContain("'Orchid Signal'")
        expect(main).toContain("'Ruby Current'")
        expect(main).toContain("name: 'Cathedral Ember'")
        expect(main).toContain("name: 'Fifths In Glass'")
        expect(main).toContain("name: 'Orchid Relay'")
        expect(main).toContain("name: 'Pelagic Bloom'")
        expect(main).toContain("name: 'Jade Lattice'")
        expect(main).toContain("name: 'Collider Bloom'")
    })

    test('compresses highlights without ACES tonemapping', () => {
        expect(shader).toContain('vec3 compressPeak(vec3 color, float limit)')
        expect(shader).toContain('c = compressPeak(c, 0.88);')
        expect(shader).toContain('c = compressPeak(c, 0.92);')
        expect(shader).not.toContain('vec3 acesToneMap')
        expect(shader).not.toContain('c = acesToneMap(c);')
    })

    test('keeps low-detail regions dark instead of flooding the frame', () => {
        expect(shader).toContain('return (1.0 - pow(abs(s), w)) * c * pow(g, d) * 3.9;')
        expect(shader).toContain('float flowBlend = clamp(iFlowVelocity * 0.52, 0.0, 0.34);')
        expect(shader).toContain('c *= mix(0.26, 1.0, detailFactor);')
        expect(shader).toContain('c = mix(c, fallback * 0.42, lowStructure * 0.07);')
    })
})
