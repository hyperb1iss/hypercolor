import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Hyperfocus',
    shader,
    {
        colorMode: combo(
            'Color Mode',
            [
                'Dopamine',
                'Serotonin',
                'Norepinephrine',
                'Melatonin',
                'Cortisol',
                'Hyperfocus',
                'Bubblegum',
                'Neon',
                'Void',
                'Mono',
            ],
            {
                default: 'Hyperfocus',
                group: 'Color',
                tooltip: 'Color palette — each named for a neurotransmitter or mental state',
            },
        ),
        energy: num('Energy', [10, 200], 140, {
            group: 'Color',
            tooltip: 'Brightness/energy of the effect',
        }),
        focusRadius: num('Focus Radius', [5, 100], 24, {
            group: 'Focus',
            tooltip: 'Radius of the sharp center region',
        }),
        focusStrength: num('Focus Strength', [0, 200], 130, {
            group: 'Focus',
            tooltip: 'Center boost/magnification',
        }),
        noise: num('Noise', [0, 200], 24, {
            group: 'Motion',
            tooltip: 'Film/noise amount, stronger in periphery',
        }),
        paralysis: num('Paralysis', [0, 100], 15, {
            group: 'Motion',
            tooltip: 'Executive dysfunction; reduces motion and spark speed',
        }),
        peripheralBlur: num('Peripheral Blur', [0, 200], 105, {
            group: 'Focus',
            tooltip: 'How much the periphery fades/softens',
        }),
        saturation: num('Saturation', [0, 200], 150, {
            group: 'Color',
            tooltip: 'Color saturation',
        }),
        sparkDensity: num('Spark Density', [0, 200], 110, {
            group: 'Motion',
            tooltip: 'Amount of dopamine sparks',
        }),
        tunnelSpeed: num('Tunnel Speed', [0, 200], 85, {
            group: 'Motion',
            tooltip: 'Motion speed of the tunnel rings',
        }),
    },
    {
        description:
            'Lock into the tunnel — dopamine sparks, breathing halos, and orbital pulses collapse the world to a single blazing point',
        presets: [
            {
                controls: {
                    colorMode: 'Dopamine',
                    energy: 195,
                    focusRadius: 18,
                    focusStrength: 190,
                    noise: 15,
                    paralysis: 0,
                    peripheralBlur: 180,
                    saturation: 180,
                    sparkDensity: 200,
                    tunnelSpeed: 140,
                },
                description:
                    'That moment the meds kick in — razor-sharp focus, sparks firing everywhere, the world narrows to a single brilliant point',
                name: 'Dopamine Rush',
            },
            {
                controls: {
                    colorMode: 'Mono',
                    energy: 45,
                    focusRadius: 65,
                    focusStrength: 40,
                    noise: 130,
                    paralysis: 95,
                    peripheralBlur: 30,
                    saturation: 60,
                    sparkDensity: 15,
                    tunnelSpeed: 15,
                },
                description:
                    'Frozen in amber — the tunnel exists but you cannot move through it, sparks barely flicker at the periphery',
                name: 'Executive Dysfunction',
            },
            {
                controls: {
                    colorMode: 'Hyperfocus',
                    energy: 160,
                    focusRadius: 6,
                    focusStrength: 200,
                    noise: 3,
                    paralysis: 10,
                    peripheralBlur: 200,
                    saturation: 150,
                    sparkDensity: 120,
                    tunnelSpeed: 60,
                },
                description:
                    'Six hours vanished — the center is absolute, peripheral reality no longer exists, time has no meaning',
                name: 'Hyperfocus Lock',
            },
            {
                controls: {
                    colorMode: 'Neon',
                    energy: 200,
                    focusRadius: 90,
                    focusStrength: 80,
                    noise: 200,
                    paralysis: 5,
                    peripheralBlur: 10,
                    saturation: 200,
                    sparkDensity: 195,
                    tunnelSpeed: 200,
                },
                description:
                    'Everything at once — the tunnel screams with neon noise, sparks saturate every frequency, no filter remains',
                name: 'Sensory Overload',
            },
            {
                controls: {
                    colorMode: 'Dopamine',
                    energy: 138,
                    focusRadius: 45,
                    focusStrength: 130,
                    noise: 30,
                    paralysis: 15,
                    peripheralBlur: 90,
                    saturation: 125,
                    sparkDensity: 100,
                    tunnelSpeed: 108,
                },
                description:
                    'The rare equilibrium — focus is wide but clear, energy is sustained, the tunnel breathes in perfect rhythm',
                name: 'Flow State',
            },
            {
                controls: {
                    colorMode: 'Serotonin',
                    energy: 100,
                    focusRadius: 60,
                    focusStrength: 80,
                    noise: 20,
                    paralysis: 30,
                    peripheralBlur: 50,
                    saturation: 140,
                    sparkDensity: 40,
                    tunnelSpeed: 40,
                },
                description:
                    'Soft warmth after the storm — the tunnel opens wide, colors cool to seafoam and teal, everything gentles',
                name: 'Serotonin Blanket',
            },
            {
                controls: {
                    colorMode: 'Norepinephrine',
                    energy: 190,
                    focusRadius: 12,
                    focusStrength: 180,
                    noise: 60,
                    paralysis: 0,
                    peripheralBlur: 160,
                    saturation: 170,
                    sparkDensity: 150,
                    tunnelSpeed: 180,
                },
                description:
                    'Deadline in five minutes — tunnel narrows to a burning point, everything outside is fire and urgency',
                name: 'Adrenaline Spike',
            },
            {
                controls: {
                    colorMode: 'Melatonin',
                    energy: 55,
                    focusRadius: 42,
                    focusStrength: 60,
                    noise: 80,
                    paralysis: 60,
                    peripheralBlur: 120,
                    saturation: 80,
                    sparkDensity: 30,
                    tunnelSpeed: 18,
                },
                description:
                    'Should have slept hours ago — melatonin whispers but the tunnel holds, deep indigo and midnight blue pulse slowly',
                name: '3 AM Doom Scroll',
            },
            {
                controls: {
                    colorMode: 'Cortisol',
                    energy: 170,
                    focusRadius: 30,
                    focusStrength: 150,
                    noise: 135,
                    paralysis: 20,
                    peripheralBlur: 140,
                    saturation: 180,
                    sparkDensity: 180,
                    tunnelSpeed: 148,
                },
                description:
                    'The same thought circling — acid green and warning amber, everything too fast and too bright, cannot look away',
                name: 'Anxiety Loop',
            },
            {
                controls: {
                    colorMode: 'Void',
                    energy: 28,
                    focusRadius: 70,
                    focusStrength: 30,
                    noise: 50,
                    paralysis: 85,
                    peripheralBlur: 180,
                    saturation: 40,
                    sparkDensity: 5,
                    tunnelSpeed: 10,
                },
                description:
                    'The dissociative pause — everything fades to near-nothing, subtle violet pulses are the only proof you still exist',
                name: 'Into the Void',
            },
            {
                controls: {
                    colorMode: 'Bubblegum',
                    energy: 145,
                    focusRadius: 55,
                    focusStrength: 100,
                    noise: 10,
                    paralysis: 5,
                    peripheralBlur: 70,
                    saturation: 190,
                    sparkDensity: 130,
                    tunnelSpeed: 110,
                },
                description:
                    'Sugar-soaked hyperdrive through a candy-coated wormhole — pink sparks burst like pop rocks dissolving on your retinas',
                name: 'Sugar Rush Protocol',
            },
            {
                controls: {
                    colorMode: 'Melatonin',
                    energy: 80,
                    focusRadius: 40,
                    focusStrength: 110,
                    noise: 90,
                    paralysis: 50,
                    peripheralBlur: 100,
                    saturation: 100,
                    sparkDensity: 55,
                    tunnelSpeed: 45,
                },
                description:
                    'Thoughts scatter like moths around a dying streetlamp — the tunnel drifts sideways, half-asleep, refusing to commit to any direction',
                name: 'Task Paralysis Limbo',
            },
        ],
    },
)
