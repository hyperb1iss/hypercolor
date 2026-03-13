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
        presets: [
            {
                name: 'Dopamine Rush',
                description: 'That moment the meds kick in — razor-sharp focus, sparks firing everywhere, the world narrows to a single brilliant point',
                controls: {
                    focusRadius: 18,
                    focusStrength: 190,
                    peripheralBlur: 180,
                    saturation: 180,
                    energy: 195,
                    sparkDensity: 200,
                    tunnelSpeed: 140,
                    paralysis: 0,
                    noise: 15,
                    colorMode: 'Dopamine',
                },
            },
            {
                name: 'Executive Dysfunction',
                description: 'Frozen in amber — the tunnel exists but you cannot move through it, sparks barely flicker at the periphery',
                controls: {
                    focusRadius: 65,
                    focusStrength: 40,
                    peripheralBlur: 30,
                    saturation: 60,
                    energy: 45,
                    sparkDensity: 20,
                    tunnelSpeed: 15,
                    paralysis: 95,
                    noise: 160,
                    colorMode: 'Mono',
                },
            },
            {
                name: 'Hyperfocus Lock',
                description: 'Six hours vanished — the center is absolute, peripheral reality no longer exists, time has no meaning',
                controls: {
                    focusRadius: 8,
                    focusStrength: 200,
                    peripheralBlur: 200,
                    saturation: 150,
                    energy: 160,
                    sparkDensity: 100,
                    tunnelSpeed: 50,
                    paralysis: 10,
                    noise: 5,
                    colorMode: 'Neon',
                },
            },
            {
                name: 'Sensory Overload',
                description: 'Everything at once — the tunnel screams with neon noise, sparks saturate every frequency, no filter remains',
                controls: {
                    focusRadius: 90,
                    focusStrength: 80,
                    peripheralBlur: 10,
                    saturation: 200,
                    energy: 200,
                    sparkDensity: 195,
                    tunnelSpeed: 200,
                    paralysis: 5,
                    noise: 200,
                    colorMode: 'Neon',
                },
            },
            {
                name: 'Flow State',
                description: 'The rare equilibrium — focus is wide but clear, energy is sustained, the tunnel breathes in perfect rhythm',
                controls: {
                    focusRadius: 45,
                    focusStrength: 130,
                    peripheralBlur: 90,
                    saturation: 120,
                    energy: 130,
                    sparkDensity: 65,
                    tunnelSpeed: 80,
                    paralysis: 15,
                    noise: 30,
                    colorMode: 'Dopamine',
                },
            },
        ],
    },
)
