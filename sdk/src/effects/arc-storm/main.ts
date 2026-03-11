import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Arc Storm',
    shader,
    {
        branches: num('Branching', [0, 100], 68, {
            step: 1,
            tooltip: 'How much the arcs split and web outward.',
        }),
        flicker: num('Instability', [0, 100], 56, {
            step: 1,
            tooltip: 'Electrical jitter, flash activity, and discharge chatter.',
        }),
        intensity: num('Core Heat', [0, 100], 72, {
            step: 1,
            tooltip: 'Brightness and white-hot core strength.',
        }),
        palette: combo(
            'Theme',
            ['Crimson Arc', 'Electric', 'Frozen', 'Phantom', 'Rosewire', 'SilkCircuit Storm', 'Solar Surge', 'Toxic'],
            {
                default: 'SilkCircuit Storm',
                tooltip:
                    'Select the discharge palette. Each theme now drives the outer glow, contrast veins, accent arcs, and core tint.',
            },
        ),
        speed: num('Charge Rate', [1, 10], 5, {
            step: 0.5,
            tooltip: 'Overall motion speed of the discharge field.',
        }),
    },
    {
        description:
            'High-voltage fractal lightning with chromatic cores, contrast-woven gradients, and a showcase house default',
    },
)
