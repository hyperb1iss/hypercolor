import { describe, expect, test } from 'bun:test'
import { injectColorPrelude, selectPreludeFunctions } from '../src/tooling/glsl-prelude'

const HEADER = '#version 300 es\nprecision highp float;\n'

function names(shader: string): string[] {
    return selectPreludeFunctions(shader).map((fn) => fn.name)
}

describe('prelude selection', () => {
    test('picks up a helper the shader calls', () => {
        expect(names(`${HEADER}void main() { vec3 c = hsv2rgb(vec3(0.5)); }`)).toEqual(['hsv2rgb'])
    })

    test('skips a helper the shader defines itself', () => {
        const shader = `${HEADER}vec3 hsv2rgb(vec3 c) {\n    return c;\n}\nvoid main() { hsv2rgb(vec3(0.5)); }`
        expect(names(shader)).toEqual([])
    })

    test('skips a definition carrying a precision qualifier', () => {
        for (const qualifier of ['lowp', 'mediump', 'highp']) {
            const shader = `${HEADER}${qualifier} vec3 hsv2rgb(vec3 c) {\n    return c;\n}\nvoid main() { hsv2rgb(vec3(0.5)); }`
            expect(names(shader)).toEqual([])
            expect(injectColorPrelude(shader).split(/\bvec3\s+hsv2rgb\s*\(/)).toHaveLength(2)
        }
    })

    test('skips a definition whose tokens straddle lines', () => {
        const shader = `${HEADER}vec3\nhsv2rgb(vec3 c)\n{\n    return c;\n}\nvoid main() { hsv2rgb(vec3(0.5)); }`
        expect(names(shader)).toEqual([])
    })

    test('a call at column zero is a call, not a definition', () => {
        const shader = `${HEADER}void main() {\nvec3 x = hsv2rgb(vec3(0.5));\n}`
        expect(names(shader)).toEqual(['hsv2rgb'])
    })

    test('a prototype alone does not count as a definition', () => {
        const shader = `${HEADER}vec3 hsv2rgb(vec3 c);\nvoid main() { hsv2rgb(vec3(0.5)); }`
        expect(names(shader)).toEqual(['hsv2rgb'])
    })

    test('keeps a local override and still supplies what it calls', () => {
        const shader = `${HEADER}vec3 saturateColor(vec3 c, float f) {\n    return hsv2rgb(rgb2hsv(c) * f);\n}\nvoid main() { saturateColor(vec3(1.0), 2.0); }`
        expect(names(shader)).toEqual(['hsv2rgb', 'rgb2hsv'])
    })

    test('pulls a helper in transitively through another helper', () => {
        expect(names(`${HEADER}void main() { saturateColor(vec3(1.0), 1.2); }`).sort()).toEqual([
            'hsv2rgb',
            'rgb2hsv',
            'saturateColor',
        ])
    })

    test('takes nothing when the shader calls nothing shared', () => {
        expect(names(`${HEADER}void main() { float x = sin(1.0); }`)).toEqual([])
    })

    test('does not confuse a substring of a helper name for a call', () => {
        expect(names(`${HEADER}void main() { myhsv2rgbHelper(vec3(0.0)); }`)).toEqual([])
    })
})

describe('prelude injection', () => {
    test('splices in after the version directive', () => {
        const injected = injectColorPrelude(`${HEADER}void main() { hsv2rgb(vec3(0.5)); }`)
        const lines = injected.split('\n')
        expect(lines[0]).toBe('#version 300 es')
        expect(injected.indexOf('vec3 hsv2rgb(vec3 c)')).toBeLessThan(injected.lastIndexOf('hsv2rgb(vec3(0.5))'))
    })

    test('declares a float precision, which the prelude bodies need', () => {
        const injected = injectColorPrelude(`${HEADER}void main() { hsv2rgb(vec3(0.5)); }`)
        expect(injected.indexOf('precision highp float;')).toBeLessThan(injected.indexOf('vec3 hsv2rgb(vec3 c)'))
    })

    test('leaves a shader that needs nothing untouched', () => {
        const shader = `${HEADER}void main() { float x = sin(1.0); }`
        expect(injectColorPrelude(shader)).toBe(shader)
    })

    test('injects exactly one definition', () => {
        const injected = injectColorPrelude(`${HEADER}void main() { hsv2rgb(vec3(0.5)); rgb2hsv(vec3(0.5)); }`)
        expect(injected.split('vec3 hsv2rgb(vec3 c)')).toHaveLength(2)
        expect(injected.split('vec3 rgb2hsv(vec3 c)')).toHaveLength(2)
    })
})
