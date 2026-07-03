import { describe, expect, test } from 'bun:test'
import { resolve } from 'node:path'

import { buildFontFaceCss, defaultFontAssetsDir, resolveFaceFontEmbedPlan } from '../src/tooling'
import type { BuildControlDef } from '../src/tooling/types'

const SDK_ROOT = resolve(import.meta.dirname, '../../..')
const FONT_ASSETS = resolve(SDK_ROOT, 'assets/fonts')

function fontControl(overrides: Partial<BuildControlDef> = {}): BuildControlDef {
    return {
        id: 'heroFont',
        label: 'Hero Font',
        type: 'combobox',
        values: ['Rajdhani', 'Orbitron'],
        ...overrides,
    }
}

describe('face font embedding', () => {
    test('collects families and weights from font controls only', () => {
        const plan = resolveFaceFontEmbedPlan([
            fontControl({ fontWeights: [500] }),
            fontControl({ id: 'uiFont', label: 'UI Font', values: ['Inter'] }),
            { id: 'dialStyle', label: 'Dial Style', type: 'combobox', values: ['Orbit', 'Split'] },
            { id: 'accent', label: 'Accent', type: 'color' },
        ])

        expect([...plan.keys()].sort()).toEqual(['Inter', 'Orbitron', 'Rajdhani'])
        expect([...(plan.get('Rajdhani') ?? [])]).toEqual([500])
        // No declared weights → the 400/600 default embed set.
        expect([...(plan.get('Inter') ?? [])].sort()).toEqual([400, 600])
    })

    test('unions weights when multiple controls share a family', () => {
        const plan = resolveFaceFontEmbedPlan([
            fontControl({ fontWeights: [300], values: ['Exo 2'] }),
            fontControl({ fontWeights: [600], id: 'uiFont', values: ['Exo 2'] }),
        ])

        expect([...(plan.get('Exo 2') ?? [])].sort()).toEqual([300, 600])
    })

    test('emits @font-face data URLs from the vendored assets', () => {
        const plan = resolveFaceFontEmbedPlan([fontControl({ fontWeights: [500] })])
        const css = buildFontFaceCss(plan, FONT_ASSETS)

        expect(css).toContain("font-family: 'Rajdhani'")
        expect(css).toContain('font-weight: 500')
        expect(css).toContain("font-family: 'Orbitron'")
        expect(css).toContain('data:font/woff2;base64,')
    })

    test('falls back to the nearest vendored weight', () => {
        // Audiowide ships a single 400 weight; a 600 request lands there.
        const plan = new Map([['Audiowide', new Set([600])]])
        const css = buildFontFaceCss(plan, FONT_ASSETS)

        expect(css).toContain("font-family: 'Audiowide'")
        expect(css).toContain('font-weight: 400')
        expect(css).not.toContain('font-weight: 600')
    })

    test('skips unvendored families without failing the build', () => {
        const plan = new Map([
            ['Comic Sans MS', new Set([400])],
            ['Rajdhani', new Set([400])],
        ])
        const css = buildFontFaceCss(plan, FONT_ASSETS)

        expect(css).not.toContain('Comic Sans')
        expect(css).toContain("font-family: 'Rajdhani'")
    })

    test('locates the vendored assets from the source tree', () => {
        expect(defaultFontAssetsDir()).toBe(FONT_ASSETS)
    })
})
