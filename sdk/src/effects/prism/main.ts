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
    presets: [
        {
            name: 'Alexandrite Kaleidoscope',
            description: 'A rare gemstone turning in candlelight — slow 6-fold symmetry with deep crystalline refractions, colors shifting between emerald and violet',
            controls: {
                palette: 'Crystal',
                speed: 2.5,
                segments: 6,
                complexity: 85,
                zoom: 45,
            },
        },
        {
            name: 'Supernova Core',
            description: 'The collapsing heart of a dying star — maximum symmetry fragments light into a white-hot mandala, ember plasma refracting through impossible geometry',
            controls: {
                palette: 'Ember',
                speed: 8,
                segments: 12,
                complexity: 95,
                zoom: 22,
            },
        },
        {
            name: 'Permafrost Fractal',
            description: 'Ice crystals under an electron microscope — minimal symmetry amplifies the alien geometry, frozen blue structures repeating into infinite depth',
            controls: {
                palette: 'Frozen',
                speed: 3,
                segments: 3,
                complexity: 60,
                zoom: 72,
            },
        },
        {
            name: 'Void Prism',
            description: 'Light bending around a singularity — midnight palette swallowing color at the edges while the center refracts in tight, hypnotic spirals',
            controls: {
                palette: 'Midnight',
                speed: 4.5,
                segments: 8,
                complexity: 78,
                zoom: 38,
            },
        },
        {
            name: 'Neon Sanctum',
            description: 'A cyberpunk cathedral rendered in pure light — fast-spinning 10-fold symmetry drenched in electric neon, maximum refraction shattering every surface',
            controls: {
                palette: 'Neon',
                speed: 7,
                segments: 10,
                complexity: 100,
                zoom: 15,
            },
        },
    ],
})
