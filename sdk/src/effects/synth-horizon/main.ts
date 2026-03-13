import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Synth Horizon',
    shader,
    {
        colorMode: combo('Color Mode', ['Color Cycle', 'Mono Neon', 'Static'], { default: 'Static', group: 'Color' }),
        cycleSpeed: num('Cycle Speed', [0, 100], 36, { group: 'Color' }),
        glow: num('Glow', [10, 100], 58, { group: 'Geometry' }),
        gridDensity: num('Grid Density', [10, 100], 62, { group: 'Geometry' }),
        palette: combo('Palette', ['Arcade Heat', 'Ice Neon', 'Midnight', 'Rink Pop', 'SilkCircuit'], {
            default: 'SilkCircuit',
            group: 'Scene',
        }),
        scene: combo('Scene', ['Arcade Carpet', 'Laser Lanes', 'Roller Grid'], { group: 'Scene' }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
    },
    {
        description: 'Crisp retro roller-rink geometry with arcade carpet motifs and neon horizon scenes',
        presets: [
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 72,
                    gridDensity: 78,
                    palette: 'Arcade Heat',
                    scene: 'Arcade Carpet',
                    speed: 4,
                },
                description:
                    'Closing time at the cabinet graveyard — CRT glow painting geometric ghosts on carpet that remembers a thousand quarters',
                name: 'Midnight Arcade',
            },
            {
                controls: {
                    colorMode: 'Color Cycle',
                    cycleSpeed: 72,
                    glow: 88,
                    gridDensity: 55,
                    palette: 'Rink Pop',
                    scene: 'Laser Lanes',
                    speed: 8,
                },
                description:
                    'A corrupted tape of a show that never aired — cycling colors bleed across laser lanes, the signal degrading beautifully',
                name: 'VHS Tracking Error',
            },
            {
                controls: {
                    colorMode: 'Mono Neon',
                    cycleSpeed: 0,
                    glow: 42,
                    gridDensity: 95,
                    palette: 'Ice Neon',
                    scene: 'Roller Grid',
                    speed: 2,
                },
                description:
                    'The substrate beneath reality — frozen cyan lattice stretching to infinity, each intersection a decision point that never fires',
                name: 'Zero-Point Grid',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 82,
                    gridDensity: 45,
                    palette: 'SilkCircuit',
                    scene: 'Laser Lanes',
                    speed: 7,
                },
                description:
                    'Chrome sun sinking behind a wireframe mountain range — magenta lanes converging at a vanishing point that never arrives',
                name: 'Outrun Horizon',
            },
            {
                controls: {
                    colorMode: 'Mono Neon',
                    cycleSpeed: 18,
                    glow: 35,
                    gridDensity: 30,
                    palette: 'Midnight',
                    scene: 'Roller Grid',
                    speed: 3,
                },
                description:
                    'Deep space navigation grid — minimal geometry pulsing in the dark, a single neon frequency marking the path forward',
                name: 'Void Protocol',
            },
        ],
    },
)
