import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Frequency Cascade',
    shader,
    {
        scene: combo('Scene', ['Cascade', 'Prism Skyline', 'Pulse Grid', 'Spectrum Tunnel'], {
            default: 'Cascade',
            group: 'Scene',
        }),
        palette: combo('Palette', ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit', 'Sunset'], {
            default: 'Aurora',
            group: 'Color',
        }),
        speed: num('Speed', [1, 10], 5, { group: 'Audio' }),
        intensity: num('Intensity', [0, 100], 75, { group: 'Audio' }),
        smoothing: num('Smoothing', [0, 100], 50, { group: 'Audio' }),
        barWidth: num('Bar Width', [0, 100], 58, { group: 'Geometry' }),
        glow: num('Glow', [0, 100], 28, { group: 'Geometry' }),
    },
    {
        audio: true,
        description:
            'Feed it sound and watch the spectrum erupt — frequency bands cascade in surging peaks of light across a pulsing audio field',
        presets: [
            {
                controls: {
                    barWidth: 75,
                    glow: 65,
                    intensity: 95,
                    palette: 'Fire',
                    scene: 'Prism Skyline',
                    smoothing: 30,
                    speed: 7,
                },
                description:
                    'Arena lights ignite on the chorus drop — towering spectral columns rise like crowd hands reaching for the hook',
                name: 'Stadium Anthem',
            },
            {
                controls: {
                    barWidth: 90,
                    glow: 85,
                    intensity: 100,
                    palette: 'Cyberpunk',
                    scene: 'Spectrum Tunnel',
                    smoothing: 70,
                    speed: 4,
                },
                description:
                    'Dubstep in a sweat-soaked basement — fat bars throbbing through a tunnel of pure low-end pressure',
                name: 'Basement Frequencies',
            },
            {
                controls: {
                    barWidth: 35,
                    glow: 40,
                    intensity: 60,
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
                    intensity: 88,
                    palette: 'Sunset',
                    scene: 'Pulse Grid',
                    smoothing: 20,
                    speed: 8,
                },
                description:
                    'Mission control monitors during a coronal mass ejection — pulsing data grid screams in solar orange and amber',
                name: 'Solar Flare Grid',
            },
            {
                controls: {
                    barWidth: 58,
                    glow: 45,
                    intensity: 78,
                    palette: 'SilkCircuit',
                    scene: 'Cascade',
                    smoothing: 60,
                    speed: 5,
                },
                description:
                    'The machine is dreaming — cascading frequency analysis rendered in electric purple and neon cyan, smooth and sentient',
                name: 'SilkCircuit Flux',
            },
            {
                controls: {
                    barWidth: 15,
                    glow: 90,
                    intensity: 70,
                    palette: 'Aurora',
                    scene: 'Spectrum Tunnel',
                    smoothing: 95,
                    speed: 2,
                },
                description:
                    'Whale song reverberates through a borealis-lit ice cave — thin spectral lines shimmer inside a tunnel of frozen green',
                name: 'Whale Song Cavern',
            },
            {
                controls: {
                    barWidth: 100,
                    glow: 20,
                    intensity: 100,
                    palette: 'Cyberpunk',
                    scene: 'Pulse Grid',
                    smoothing: 5,
                    speed: 10,
                },
                description:
                    'A Tokyo arcade cabinet overloads on drum-and-bass — razor-wide columns slam the grid in magenta and cyan at 174 BPM',
                name: 'Akihabara Overload',
            },
            {
                controls: {
                    barWidth: 42,
                    glow: 72,
                    intensity: 55,
                    palette: 'Ice',
                    scene: 'Prism Skyline',
                    smoothing: 75,
                    speed: 3,
                },
                description:
                    'A glass city skyline refracts ambient piano into pale blue towers — each note lifts a column of cold crystal light',
                name: 'Glass City Lullaby',
            },
        ],
    },
)
