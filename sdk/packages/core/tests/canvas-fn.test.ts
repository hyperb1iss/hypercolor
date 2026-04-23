import { afterEach, describe, expect, test } from 'bun:test'

import { combo, paletteControl } from '../src/controls/specs'
import { __testing } from '../src/effects/canvas-fn'

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

describe('canvas palette control resolution', () => {
    test('keeps explicit combo palette controls as strings', () => {
        setTestWindow({ palette: 'Aurora' })

        const controls = __testing.resolveCanvasControls({
            palette: combo('Palette', ['SilkCircuit', 'Aurora'], { default: 'SilkCircuit' }),
        })
        const values = __testing.resolveValues(controls, new Map())

        expect(values.palette).toBe('Aurora')
    })

    test('treats palette shorthand as a plain combobox', () => {
        setTestWindow({ palette: 'Aurora' })

        const controls = __testing.resolveCanvasControls({
            palette: ['SilkCircuit', 'Aurora'],
        })
        const values = __testing.resolveValues(controls, new Map())

        expect(values.palette).toBe('Aurora')
    })

    test('resolves explicit palette controls as palette functions', () => {
        setTestWindow({ palette: 'Aurora' })

        const controls = __testing.resolveCanvasControls({
            palette: paletteControl('Palette', ['SilkCircuit', 'Aurora'], { default: 'SilkCircuit' }),
        })
        const values = __testing.resolveValues(controls, new Map())

        expect(typeof values.palette).toBe('function')
    })
})
