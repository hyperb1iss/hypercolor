import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Prism', shader, {
    palette: combo('Theme', ['Crystal', 'Ember', 'Frozen', 'Midnight', 'Neon', 'SilkCircuit'], {
        default: 'SilkCircuit',
        tooltip: 'Select the prism color family.',
    }),
    speed: num('Rotation', [1, 10], 4, {
        step: 0.5,
        tooltip: 'Speed of the global prism rotation and color drift.',
    }),
    segments: num('Symmetry', [3, 12], 8, {
        step: 1,
        tooltip: 'Number of kaleidoscope slices. The shader quantizes this to whole numbers.',
    }),
    complexity: num('Refraction', [0, 100], 72, {
        step: 1,
        tooltip: 'Layered crystalline detail and contour density.',
    }),
    zoom: num('Scale', [0, 100], 38, {
        step: 1,
        tooltip: 'Tightens or widens the folded prism pattern.',
    }),
}, {
    description: 'Sharper kaleidoscopic refraction with explicit symmetry, detail, and scale control',
})
