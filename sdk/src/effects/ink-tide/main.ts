import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Ink Tide', shader, {
    palette: combo('Theme', ['Abyss', 'Arctic', 'Molten', 'Phantom', 'Poison', 'Sakura'], {
        default: 'Sakura',
        tooltip: 'Choose the ink palette. Sakura lifts the default into a brighter bloom.',
        group: 'Color',
    }),
    speed: num('Current Speed', [1, 10], 5, {
        step: 0.5,
        tooltip: 'Overall drift speed of the liquid field.',
        group: 'Motion',
    }),
    flow: num('Fold Depth', [0, 100], 78, {
        step: 1,
        tooltip: 'Strength of the large folding motion and exposure lift.',
        group: 'Motion',
    }),
    turbulence: num('Detail', [0, 100], 64, {
        step: 1,
        tooltip: 'Amount of fine swirling structure in the ink.',
        group: 'Motion',
    }),
    saturation: num('Color Lift', [0, 100], 92, {
        step: 1,
        tooltip: 'How vivid the ink stays before drifting toward grayscale.',
        group: 'Color',
    }),
}, {
    description: 'Liquid neon ink blooms with deeper folds, brighter defaults, and a richer color lift',
    presets: [
        {
            name: 'Abyssal Bioluminescence',
            description: 'Cold ink plumes drift through crushing deep-sea pressure — faint living light pulses in the mariana dark',
            controls: { palette: 'Abyss', speed: 2.5, flow: 42, turbulence: 88, saturation: 68 },
        },
        {
            name: 'Volcanic Calligraphy',
            description: 'Molten pigment sears across obsidian — each fold a brushstroke of liquid basalt cooling into glass',
            controls: { palette: 'Molten', speed: 7.5, flow: 95, turbulence: 72, saturation: 100 },
        },
        {
            name: 'Cherry Blossom Monsoon',
            description: 'Silk petals dissolve in warm rain, staining the current with delicate washes of living pink',
            controls: { palette: 'Sakura', speed: 4, flow: 60, turbulence: 45, saturation: 85 },
        },
        {
            name: 'Cephalopod Camouflage',
            description: 'Chromatophores fire in nervous cascades — ink sacs rupture and bloom through ice-cold polar currents',
            controls: { palette: 'Arctic', speed: 8, flow: 85, turbulence: 100, saturation: 55 },
        },
        {
            name: 'Toxic Estuary',
            description: 'Industrial runoff meets brackish tide — phosphorescent poison seeps through stagnant folds of dead water',
            controls: { palette: 'Poison', speed: 3, flow: 30, turbulence: 78, saturation: 76 },
        },
    ],
})
