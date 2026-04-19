import { describe, expect, test } from 'bun:test'

import { combo, font } from '../src/controls/specs'
import { __testing } from '../src/faces/face-fn'

describe('face font loading', () => {
    test('loads only the active font selections', () => {
        const controls = __testing.resolveFaceControls({
            heroFont: font('Hero Font', 'Rajdhani', { families: ['Rajdhani', 'Orbitron', 'Space Mono'] }),
            meterStyle: combo('Meter Style', ['Halo', 'Vector', 'Scope']),
            uiFont: font('UI Font', 'Inter', { families: ['Inter', 'DM Sans', 'Sora'] }),
        })

        const fontControls = __testing.resolveFaceFontControls(controls)
        const families = __testing.resolveFaceFontFamilies(fontControls, {
            heroFont: 'Orbitron',
            meterStyle: 'Scope',
            uiFont: 'DM Sans',
        })

        expect(families).toEqual(['Orbitron', 'DM Sans'])
    })

    test('falls back to defaults and dedupes shared fonts', () => {
        const controls = __testing.resolveFaceControls({
            headlineFont: font('Headline Font', 'Rajdhani', { families: ['Rajdhani', 'Orbitron'] }),
            uiFont: font('UI Font', 'Rajdhani', { families: ['Rajdhani', 'Inter'] }),
        })

        const fontControls = __testing.resolveFaceFontControls(controls)
        const families = __testing.resolveFaceFontFamilies(fontControls, {
            headlineFont: '',
            uiFont: 'Rajdhani',
        })

        expect(families).toEqual(['Rajdhani'])
    })
})
