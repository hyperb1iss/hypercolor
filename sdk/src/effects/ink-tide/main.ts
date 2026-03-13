import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Ink Tide',
    shader,
    {
        flow: num('Fold Depth', [0, 100], 70, {
            group: 'Motion',
            step: 1,
            tooltip: 'How far the ink spreads — low values leave more dark water visible.',
        }),
        palette: combo('Theme', ['Abyss', 'Arctic', 'Molten', 'Phantom', 'Poison', 'Sakura'], {
            default: 'Sakura',
            group: 'Color',
            tooltip: 'Three independent ink colors per theme.',
        }),
        saturation: num('Color Lift', [0, 100], 85, {
            group: 'Color',
            step: 1,
            tooltip: 'Push ink colors toward full vividness.',
        }),
        speed: num('Current Speed', [1, 10], 5, {
            group: 'Motion',
            step: 0.5,
            tooltip: 'Drift speed of the ink fields.',
        }),
        turbulence: num('Detail', [0, 100], 60, {
            group: 'Motion',
            step: 1,
            tooltip: 'Spatial complexity — higher values create finer ink structures.',
        }),
    },
    {
        description:
            'Three independent ink drops bleed through dark water — each color spreads on its own warp field, meeting at luminous boundaries',
        presets: [
            {
                controls: { flow: 40, palette: 'Abyss', saturation: 75, speed: 2.5, turbulence: 80 },
                description:
                    'Teal and cyan ink plumes drift apart in crushing deep-sea pressure — dark water dominates, punctuated by bioluminescent tendrils',
                name: 'Abyssal Bioluminescence',
            },
            {
                controls: { flow: 90, palette: 'Molten', saturation: 100, speed: 7, turbulence: 65 },
                description:
                    'Three rivers of molten pigment — red, orange, amber — collide and fold over volcanic black, overlaps burning white-hot',
                name: 'Volcanic Calligraphy',
            },
            {
                controls: { flow: 55, palette: 'Sakura', saturation: 90, speed: 4, turbulence: 45 },
                description:
                    'Pink, magenta, and rose ink dissolve in separate currents through dark plum water — petals of color drifting apart',
                name: 'Cherry Blossom Monsoon',
            },
            {
                controls: { flow: 75, palette: 'Arctic', saturation: 65, speed: 8, turbulence: 95 },
                description:
                    'Blue, cyan, and ice-white inks fracture into turbulent filaments — chromatophore cascades through freezing polar currents',
                name: 'Cephalopod Camouflage',
            },
            {
                controls: { flow: 35, palette: 'Poison', saturation: 80, speed: 3, turbulence: 70 },
                description:
                    'Acid green, chartreuse, and toxic yellow seep through stagnant black water — distinct poison streams barely touching',
                name: 'Toxic Estuary',
            },
            {
                controls: { flow: 100, palette: 'Phantom', saturation: 40, speed: 1.5, turbulence: 20 },
                description:
                    'Violet and lavender inks exhale in slow motion through void — nearly merged but still shifting in desaturated phantom layers',
                name: 'Séance Smoke',
            },
            {
                controls: { flow: 80, palette: 'Sakura', saturation: 100, speed: 9.5, turbulence: 90 },
                description:
                    'All three sakura inks at terminal velocity — overlapping zones flare white-hot as the currents rip into turbulent chaos',
                name: 'Dopamine Rush',
            },
        ],
    },
)
