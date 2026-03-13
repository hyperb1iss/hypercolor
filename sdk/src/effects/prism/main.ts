import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Prism',
    shader,
    {
        palette: combo('Theme', ['Crystal', 'Ember', 'Frozen', 'Midnight', 'Neon', 'SilkCircuit'], {
            default: 'SilkCircuit',
            tooltip: 'Select the prism color family.',
            group: 'Color',
        }),
        speed: num('Rotation', [1, 10], 4, {
            step: 0.5,
            tooltip: 'Speed of the global prism rotation and color drift.',
            group: 'Motion',
        }),
        segments: num('Symmetry', [3, 12], 8, {
            step: 1,
            tooltip: 'Number of kaleidoscope slices. The shader quantizes this to whole numbers.',
            group: 'Geometry',
        }),
        zoom: num('Scale', [0, 100], 38, {
            step: 1,
            tooltip: 'Tightens or widens the folded prism pattern.',
            group: 'Geometry',
        }),
        complexity: num('Refraction', [0, 100], 72, {
            step: 1,
            tooltip: 'Layered crystalline detail and contour density.',
            group: 'Geometry',
        }),
    },
    {
        description:
            'Light shatters through a prismatic lattice — kaleidoscopic refraction splits and recombines in crystalline symmetric patterns',
        presets: [
            {
                controls: {
                    complexity: 85,
                    palette: 'Crystal',
                    segments: 6,
                    speed: 2.5,
                    zoom: 45,
                },
                description:
                    'A rare gemstone turning in candlelight — slow 6-fold symmetry with deep crystalline refractions, colors shifting between emerald and violet',
                name: 'Alexandrite Kaleidoscope',
            },
            {
                controls: {
                    complexity: 95,
                    palette: 'Ember',
                    segments: 12,
                    speed: 8,
                    zoom: 22,
                },
                description:
                    'The collapsing heart of a dying star — maximum symmetry fragments light into a white-hot mandala, ember plasma refracting through impossible geometry',
                name: 'Supernova Core',
            },
            {
                controls: {
                    complexity: 60,
                    palette: 'Frozen',
                    segments: 3,
                    speed: 3,
                    zoom: 72,
                },
                description:
                    'Ice crystals magnified under an electron microscope — three-fold alien geometry locks frozen blue structures into infinite recursive depth',
                name: 'Permafrost Fractal',
            },
            {
                controls: {
                    complexity: 78,
                    palette: 'Midnight',
                    segments: 8,
                    speed: 4.5,
                    zoom: 38,
                },
                description:
                    'Light bending around a singularity — midnight palette swallowing color at the edges while the center refracts in tight, hypnotic spirals',
                name: 'Void Prism',
            },
            {
                controls: {
                    complexity: 100,
                    palette: 'Neon',
                    segments: 10,
                    speed: 7,
                    zoom: 15,
                },
                description:
                    'A cyberpunk cathedral rendered in pure light — fast-spinning 10-fold symmetry drenched in electric neon, maximum refraction shattering every surface',
                name: 'Neon Sanctum',
            },
            {
                controls: {
                    complexity: 42,
                    palette: 'SilkCircuit',
                    segments: 4,
                    speed: 1.5,
                    zoom: 90,
                },
                description:
                    'A stained glass window turns in a forgotten chapel — four-fold symmetry holds electric purple and cyan in slow, meditative orbit',
                name: 'Cathedral of Circuits',
            },
            {
                controls: {
                    complexity: 88,
                    palette: 'Ember',
                    segments: 5,
                    speed: 9.5,
                    zoom: 5,
                },
                description:
                    'Pentagonal fire collapses inward at terminal velocity — incandescent geometry folding and unfolding in a white-hot maelstrom',
                name: 'Furnace Mandala',
            },
        ],
    },
)
