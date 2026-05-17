import { afterEach, describe, expect, test } from 'bun:test'

import { asset, combo, paletteControl } from '../src/controls/specs'
import { __testing } from '../src/effects/effect-fn'

const originalWindow = (globalThis as { window?: Record<string, unknown> }).window

function setTestWindow(values: Record<string, unknown>): void {
    ;(globalThis as { window?: Record<string, unknown> }).window = values
}

afterEach(() => {
    if (originalWindow) {
        setTestWindow({ ...originalWindow })
        return
    }

    delete (globalThis as { window?: Record<string, unknown> }).window
})

describe('effect palette control resolution', () => {
    test('keeps plain combo palette controls string-valued in runtime control state', () => {
        setTestWindow({ palette: 'Aurora' })

        const controls = __testing.resolveControls({
            palette: combo('Palette', ['SilkCircuit', 'Aurora'], { default: 'SilkCircuit' }),
        })
        const values = __testing.resolveControlValues(controls)

        expect(values.palette).toBe('Aurora')
    })

    test('requires explicit palette controls for palette index transforms', () => {
        setTestWindow({ mode: 'Wave', palette: 'Aurora' })

        const controls = __testing.resolveControls({
            mode: combo('Mode', ['Pulse', 'Wave']),
            palette: paletteControl('Palette', ['SilkCircuit', 'Aurora'], { default: 'SilkCircuit' }),
        })
        const values = __testing.resolveControlValues(controls)

        const mode = controls.find((control) => control.key === 'mode')
        const palette = controls.find((control) => control.key === 'palette')

        expect(mode?.isPaletteTransform).toBe(false)
        expect(palette?.isPaletteTransform).toBe(true)
        expect(values.mode).toBe('Wave')
        expect(values.palette).toBe(1)
    })

    test('keeps asset controls string-valued and records media kind', () => {
        setTestWindow({ logo: 'asset_01JZ' })

        const controls = __testing.resolveControls({
            logo: asset('Logo', 'image'),
        })
        const values = __testing.resolveControlValues(controls)

        expect(controls[0]?.spec.__type).toBe('asset')
        expect(controls[0]?.spec.meta.mediaKind).toBe('image')
        expect(values.logo).toBe('asset_01JZ')
    })
})
