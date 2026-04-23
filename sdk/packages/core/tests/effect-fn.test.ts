import { describe, expect, test } from 'bun:test'

import { combo, paletteControl } from '../src/controls/specs'
import { __testing } from '../src/effects/effect-fn'

describe('effect palette control resolution', () => {
    test('keeps plain combo palette controls string-valued in control state', () => {
        const controls = __testing.resolveControls({
            palette: combo('Palette', ['SilkCircuit', 'Aurora'], { default: 'SilkCircuit' }),
        })

        expect(controls[0]?.isPaletteTransform).toBe(false)
    })

    test('requires explicit palette controls for palette index transforms', () => {
        const controls = __testing.resolveControls({
            mode: combo('Mode', ['Pulse', 'Wave']),
            palette: paletteControl('Palette', ['SilkCircuit', 'Aurora'], { default: 'SilkCircuit' }),
        })

        const mode = controls.find((control) => control.key === 'mode')
        const palette = controls.find((control) => control.key === 'palette')

        expect(mode?.isPaletteTransform).toBe(false)
        expect(palette?.isPaletteTransform).toBe(true)
    })
})
