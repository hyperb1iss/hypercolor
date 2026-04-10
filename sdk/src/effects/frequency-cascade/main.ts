import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Frequency Cascade',
    shader,
    {
        scene: combo('Scene', ['Cascade', 'Mirror', 'Horizon', 'Tunnel'], {
            default: 'Cascade',
            group: 'Scene',
        }),
        palette: combo('Palette', ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit', 'Sunset'], {
            default: 'Aurora',
            group: 'Color',
        }),
        speed: num('Speed', [1, 10], 5, { group: 'Audio' }),
        intensity: num('Intensity', [0, 100], 80, { group: 'Audio' }),
        smoothing: num('Smoothing', [0, 100], 60, { group: 'Audio' }),
        barWidth: num('Bar Width', [0, 100], 55, { group: 'Geometry' }),
        glow: num('Glow', [0, 100], 45, { group: 'Geometry' }),
    },
    {
        audio: true,
        description:
            'Feed it sound and watch the spectrum breathe — frequency bands rise in poised columns of light across a stable, luminous field',
        presets: [
            {
                controls: {
                    barWidth: 70,
                    glow: 60,
                    intensity: 92,
                    palette: 'Fire',
                    scene: 'Horizon',
                    smoothing: 45,
                    speed: 7,
                },
                description:
                    'Arena lights ignite on the chorus drop — towering spectral columns rise from a blazing horizon like crowd hands reaching for the hook',
                name: 'Stadium Anthem',
            },
            {
                controls: {
                    barWidth: 85,
                    glow: 75,
                    intensity: 95,
                    palette: 'Cyberpunk',
                    scene: 'Tunnel',
                    smoothing: 60,
                    speed: 4,
                },
                description:
                    'Dubstep in a sweat-soaked basement — fat bars throb through a tunnel of pure low-end pressure',
                name: 'Basement Frequencies',
            },
            {
                controls: {
                    barWidth: 30,
                    glow: 40,
                    intensity: 58,
                    palette: 'Ice',
                    scene: 'Cascade',
                    smoothing: 80,
                    speed: 3,
                },
                description:
                    'Classical piano refracted through ice prisms — each note a delicate falling column of frozen light',
                name: 'Crystal Cascade',
            },
            {
                controls: {
                    barWidth: 50,
                    glow: 55,
                    intensity: 85,
                    palette: 'Sunset',
                    scene: 'Horizon',
                    smoothing: 35,
                    speed: 7,
                },
                description:
                    'Mission control monitors during a coronal mass ejection — bars surge from a burning horizon in solar orange and amber',
                name: 'Solar Flare Grid',
            },
            {
                controls: {
                    barWidth: 55,
                    glow: 50,
                    intensity: 78,
                    palette: 'SilkCircuit',
                    scene: 'Cascade',
                    smoothing: 65,
                    speed: 5,
                },
                description:
                    'The machine is dreaming — cascading frequency analysis rendered in electric purple and neon cyan, smooth and sentient',
                name: 'SilkCircuit Flux',
            },
            {
                controls: {
                    barWidth: 25,
                    glow: 85,
                    intensity: 65,
                    palette: 'Aurora',
                    scene: 'Tunnel',
                    smoothing: 88,
                    speed: 2,
                },
                description:
                    'Whale song reverberates through a borealis-lit ice cave — thin spectral lines shimmer inside a tunnel of frozen green',
                name: 'Whale Song Cavern',
            },
            {
                controls: {
                    barWidth: 95,
                    glow: 35,
                    intensity: 100,
                    palette: 'Cyberpunk',
                    scene: 'Mirror',
                    smoothing: 25,
                    speed: 10,
                },
                description:
                    'A Tokyo arcade cabinet overloads on drum-and-bass — razor-wide columns slam the mirror line in magenta and cyan at 174 BPM',
                name: 'Akihabara Overload',
            },
            {
                controls: {
                    barWidth: 42,
                    glow: 70,
                    intensity: 55,
                    palette: 'Ice',
                    scene: 'Horizon',
                    smoothing: 80,
                    speed: 3,
                },
                description:
                    'A glass city skyline refracts ambient piano into pale blue towers — each note lifts a column of cold crystal light from still water',
                name: 'Glass City Lullaby',
            },
        ],
    },
)
