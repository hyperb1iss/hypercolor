import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'ADHD Hyperfocus',
    shader,
    {
        focusRadius: num('Focus Radius', [5, 100], 28, {
            tooltip: 'Radius of the sharp center region',
        }),
        focusStrength: num('Focus Strength', [0, 200], 120, {
            tooltip: 'Center boost/magnification',
        }),
        peripheralBlur: num('Peripheral Blur', [0, 200], 60, {
            tooltip: 'How much the periphery fades/softens',
        }),
        saturation: num('Saturation', [0, 200], 120, {
            tooltip: 'Color saturation',
        }),
        energy: num('Energy', [10, 200], 120, {
            tooltip: 'Brightness/energy of the effect',
        }),
        sparkDensity: num('Spark Density', [0, 200], 80, {
            tooltip: 'Amount of dopamine sparks',
        }),
        tunnelSpeed: num('Tunnel Speed', [0, 200], 70, {
            tooltip: 'Motion speed of the tunnel rings',
        }),
        paralysis: num('Paralysis', [0, 100], 25, {
            tooltip: 'Executive dysfunction; reduces motion and spark speed',
        }),
        noise: num('Noise', [0, 200], 40, {
            tooltip: 'Film/noise amount, stronger in periphery',
        }),
        colorMode: combo('Color Mode', ['Dopamine', 'Mono', 'Neon'], {
            default: 'Dopamine',
            tooltip: 'Color palette',
        }),
    },
    {
        description:
            'Tunnel vision with hyperfocused center and dopamine sparks. Peripheral fade, paralysis control.',
    },
)
