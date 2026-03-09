import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Ink Tide', shader, {
    palette: combo('Theme', ['Abyss', 'Sakura', 'Poison', 'Molten', 'Arctic', 'Phantom'], {
        default: 'Sakura',
        tooltip: 'Choose the ink palette. Sakura lifts the default into a brighter bloom.',
    }),
    speed: num('Current Speed', [1, 10], 5, {
        step: 0.5,
        tooltip: 'Overall drift speed of the liquid field.',
    }),
    flow: num('Fold Depth', [0, 100], 78, {
        step: 1,
        tooltip: 'Strength of the large folding motion and exposure lift.',
    }),
    turbulence: num('Detail', [0, 100], 64, {
        step: 1,
        tooltip: 'Amount of fine swirling structure in the ink.',
    }),
    saturation: num('Color Lift', [0, 100], 92, {
        step: 1,
        tooltip: 'How vivid the ink stays before drifting toward grayscale.',
    }),
}, {
    description: 'Liquid neon ink blooms with deeper folds, brighter defaults, and a richer color lift',
})
