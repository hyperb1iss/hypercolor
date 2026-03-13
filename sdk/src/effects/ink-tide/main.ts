import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Ink Tide',
    shader,
    {
        flow: num('Fold Depth', [0, 100], 78, {
            group: 'Motion',
            step: 1,
            tooltip: 'Strength of the large folding motion and exposure lift.',
        }),
        palette: combo('Theme', ['Abyss', 'Arctic', 'Molten', 'Phantom', 'Poison', 'Sakura'], {
            default: 'Sakura',
            group: 'Color',
            tooltip: 'Choose the ink palette. Sakura lifts the default into a brighter bloom.',
        }),
        saturation: num('Color Lift', [0, 100], 92, {
            group: 'Color',
            step: 1,
            tooltip: 'How vivid the ink stays before drifting toward grayscale.',
        }),
        speed: num('Current Speed', [1, 10], 5, {
            group: 'Motion',
            step: 0.5,
            tooltip: 'Overall drift speed of the liquid field.',
        }),
        turbulence: num('Detail', [0, 100], 64, {
            group: 'Motion',
            step: 1,
            tooltip: 'Amount of fine swirling structure in the ink.',
        }),
    },
    {
        description: 'Liquid neon ink blooms with deeper folds, brighter defaults, and a richer color lift',
        presets: [
            {
                controls: { flow: 42, palette: 'Abyss', saturation: 68, speed: 2.5, turbulence: 88 },
                description:
                    'Cold ink plumes drift through crushing deep-sea pressure — faint living light pulses in the mariana dark',
                name: 'Abyssal Bioluminescence',
            },
            {
                controls: { flow: 95, palette: 'Molten', saturation: 100, speed: 7.5, turbulence: 72 },
                description:
                    'Molten pigment sears across obsidian — each fold a brushstroke of liquid basalt cooling into glass',
                name: 'Volcanic Calligraphy',
            },
            {
                controls: { flow: 60, palette: 'Sakura', saturation: 85, speed: 4, turbulence: 45 },
                description:
                    'Silk petals dissolve in warm rain, staining the current with delicate washes of living pink',
                name: 'Cherry Blossom Monsoon',
            },
            {
                controls: { flow: 85, palette: 'Arctic', saturation: 55, speed: 8, turbulence: 100 },
                description:
                    'Chromatophores fire in nervous cascades — ink sacs rupture and bloom through ice-cold polar currents',
                name: 'Cephalopod Camouflage',
            },
            {
                controls: { flow: 30, palette: 'Poison', saturation: 76, speed: 3, turbulence: 78 },
                description:
                    'Industrial runoff meets brackish tide — phosphorescent poison seeps through stagnant folds of dead water',
                name: 'Toxic Estuary',
            },
        ],
    },
)
