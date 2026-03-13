import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Synth Horizon', shader, {
    scene: combo('Scene', ['Arcade Carpet', 'Laser Lanes', 'Roller Grid'], { group: 'Scene' }),
    palette: combo('Palette', ['Arcade Heat', 'Ice Neon', 'Midnight', 'Rink Pop', 'SilkCircuit'], { default: 'SilkCircuit', group: 'Scene' }),
    colorMode: combo('Color Mode', ['Color Cycle', 'Mono Neon', 'Static'], { default: 'Static', group: 'Color' }),
    cycleSpeed: num('Cycle Speed', [0, 100], 36, { group: 'Color' }),
    speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
    gridDensity: num('Grid Density', [10, 100], 62, { group: 'Geometry' }),
    glow: num('Glow', [10, 100], 58, { group: 'Geometry' }),
}, {
    description: 'Crisp retro roller-rink geometry with arcade carpet motifs and neon horizon scenes',
    presets: [
        {
            name: 'Midnight Arcade',
            description: 'Closing time at the cabinet graveyard — CRT glow painting geometric ghosts on carpet that remembers a thousand quarters',
            controls: {
                scene: 'Arcade Carpet',
                speed: 4,
                gridDensity: 78,
                glow: 72,
                palette: 'Arcade Heat',
                colorMode: 'Static',
                cycleSpeed: 0,
            },
        },
        {
            name: 'VHS Tracking Error',
            description: 'A corrupted tape of a show that never aired — cycling colors bleed across laser lanes, the signal degrading beautifully',
            controls: {
                scene: 'Laser Lanes',
                speed: 8,
                gridDensity: 55,
                glow: 88,
                palette: 'Rink Pop',
                colorMode: 'Color Cycle',
                cycleSpeed: 72,
            },
        },
        {
            name: 'Zero-Point Grid',
            description: 'The substrate beneath reality — frozen cyan lattice stretching to infinity, each intersection a decision point that never fires',
            controls: {
                scene: 'Roller Grid',
                speed: 2,
                gridDensity: 95,
                glow: 42,
                palette: 'Ice Neon',
                colorMode: 'Mono Neon',
                cycleSpeed: 0,
            },
        },
        {
            name: 'Outrun Horizon',
            description: 'Chrome sun sinking behind a wireframe mountain range — magenta lanes converging at a vanishing point that never arrives',
            controls: {
                scene: 'Laser Lanes',
                speed: 7,
                gridDensity: 45,
                glow: 82,
                palette: 'SilkCircuit',
                colorMode: 'Static',
                cycleSpeed: 0,
            },
        },
        {
            name: 'Void Protocol',
            description: 'Deep space navigation grid — minimal geometry pulsing in the dark, a single neon frequency marking the path forward',
            controls: {
                scene: 'Roller Grid',
                speed: 3,
                gridDensity: 30,
                glow: 35,
                palette: 'Midnight',
                colorMode: 'Mono Neon',
                cycleSpeed: 18,
            },
        },
    ],
})
