import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Arc Storm', shader, {
    palette: combo('Theme', ['Electric', 'SilkCircuit Storm', 'Crimson Arc', 'Toxic', 'Frozen', 'Phantom'], {
        default: 'SilkCircuit Storm',
        tooltip: 'Select the arc color family. SilkCircuit Storm is the showcase default.',
    }),
    speed: num('Charge Rate', [1, 10], 5, {
        step: 0.5,
        tooltip: 'Overall motion speed of the discharge field.',
    }),
    intensity: num('Core Heat', [0, 100], 72, {
        step: 1,
        tooltip: 'Brightness and white-hot core strength.',
    }),
    branches: num('Branching', [0, 100], 68, {
        step: 1,
        tooltip: 'How much the arcs split and web outward.',
    }),
    flicker: num('Instability', [0, 100], 56, {
        step: 1,
        tooltip: 'Electrical jitter, flash activity, and discharge chatter.',
    }),
}, {
    description: 'High-voltage fractal lightning with tunable branching, hot cores, and a showcase house default',
})
