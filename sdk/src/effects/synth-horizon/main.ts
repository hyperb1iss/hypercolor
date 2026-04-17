import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Synth Horizon',
    shader,
    {
        colorMode: combo('Color Mode', ['Static', 'Color Cycle', 'Mono Neon', 'Evolve'], {
            default: 'Static',
            group: 'Color',
        }),
        cycleSpeed: num('Cycle Speed', [0, 100], 0, { group: 'Color' }),
        glow: num('Glow', [0, 100], 65, { group: 'Atmosphere' }),
        motion: combo('Motion Style', ['Cruise', 'Serpentine', 'Solar Pulse', 'Stargaze', 'Hyperdrive'], {
            default: 'Cruise',
            group: 'Motion',
        }),
        mountains: num('Mountains', [0, 100], 55, { group: 'Scene' }),
        palette: combo(
            'Palette',
            [
                'Nightcall',
                'Dusk Drive',
                'Miami Chrome',
                'Tron Coast',
                'Blood Dragon',
                'Vapor Sunset',
                'Midnight Coast',
                'SilkCircuit',
                'Rose Gold',
                'Toxic Rain',
                'Arctic Mirage',
                'Sunset Strip',
            ],
            { default: 'SilkCircuit', group: 'Color' },
        ),
        scene: combo('Scene', ['Open Road', 'Coastal', 'Ridge Run', 'Canyon', 'Twin Moons', 'Aurora Peak'], {
            default: 'Open Road',
            group: 'Scene',
        }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        sunSize: num('Sun Size', [0, 100], 55, { group: 'Scene' }),
    },
    {
        description:
            'A chrome sun sinks behind a scrolling wireframe horizon. Six scenes from winding canyons to twin-moon coasts, five motion personalities, twelve palettes of pure 1984 synthwave geometry.',
        presets: [
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 65,
                    motion: 'Cruise',
                    mountains: 45,
                    palette: 'SilkCircuit',
                    scene: 'Open Road',
                    speed: 5,
                    sunSize: 55,
                },
                description:
                    'The signature run. Coral sun bleeds over a neon cyan grid, amethyst sky holding its breath before midnight.',
                name: 'Outrun Horizon',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 78,
                    motion: 'Solar Pulse',
                    mountains: 40,
                    palette: 'Nightcall',
                    scene: 'Open Road',
                    speed: 4,
                    sunSize: 60,
                },
                description:
                    'Kavinsky radio static at 3am. Yellow sun breathes through its chrome bands, magenta grid rolling toward a city that burned down last year.',
                name: 'Midnight Arcade',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 58,
                    motion: 'Serpentine',
                    mountains: 78,
                    palette: 'Dusk Drive',
                    scene: 'Ridge Run',
                    speed: 3,
                    sunSize: 52,
                },
                description:
                    'Timecop1983 coasting down a mountain pass. Honey sun, lavender ridges, the road itself writing lazy S-curves through the dusk.',
                name: 'Dusk Drive',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 92,
                    motion: 'Hyperdrive',
                    mountains: 100,
                    palette: 'Blood Dragon',
                    scene: 'Ridge Run',
                    speed: 8,
                    sunSize: 50,
                },
                description:
                    'Year 2007 XXII. Cybernetic mountains flash past at warp speed, amber sun trembling as the horizon strobes crimson.',
                name: 'Blood Dragon',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 74,
                    motion: 'Stargaze',
                    mountains: 25,
                    palette: 'Vapor Sunset',
                    scene: 'Open Road',
                    speed: 2,
                    sunSize: 82,
                },
                description:
                    'Cassette-deck chillwave dream. Fat pastel sun, stars sliding across a lavender sky, horizon shimmering like a memory trying to crystallize.',
                name: 'Vapor Sunset',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 68,
                    motion: 'Solar Pulse',
                    mountains: 0,
                    palette: 'Tron Coast',
                    scene: 'Coastal',
                    speed: 6,
                    sunSize: 70,
                },
                description:
                    'A white chrome disc rises over a cyan digital sea. Bands roll upward through the sun like the first line of code ever written.',
                name: 'Tron Coast',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 36,
                    motion: 'Cruise',
                    mountains: 0,
                    palette: 'Midnight Coast',
                    scene: 'Coastal',
                    speed: 1,
                    sunSize: 42,
                },
                description:
                    'Frozen highway under FM-84 satellites. A minimal neon pulse crawls through the absolute cold, destination unknown.',
                name: 'Permafrost Causeway',
            },
            {
                controls: {
                    colorMode: 'Color Cycle',
                    cycleSpeed: 68,
                    glow: 94,
                    motion: 'Hyperdrive',
                    mountains: 50,
                    palette: 'Miami Chrome',
                    scene: 'Open Road',
                    speed: 8,
                    sunSize: 62,
                },
                description:
                    'A corrupted VHS tape of a show that never aired. Cycling hues bleed across the grid at warp speed, the signal degrading beautifully.',
                name: 'VHS Tracking Error',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 72,
                    motion: 'Serpentine',
                    mountains: 85,
                    palette: 'Rose Gold',
                    scene: 'Canyon',
                    speed: 4,
                    sunSize: 58,
                },
                description:
                    'A champagne sun sinks into a rose-gold gorge. Canyon walls burn soft pink as the road threads between them.',
                name: 'Rose Gold Canyon',
            },
            {
                controls: {
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    glow: 82,
                    motion: 'Solar Pulse',
                    mountains: 0,
                    palette: 'Arctic Mirage',
                    scene: 'Twin Moons',
                    speed: 3,
                    sunSize: 65,
                },
                description:
                    'An ice world at the edge of nowhere. Two crystalline moons hold each other up, their cold chrome bands breathing in slow harmony.',
                name: 'Twin Moons Ascension',
            },
            {
                controls: {
                    colorMode: 'Evolve',
                    cycleSpeed: 40,
                    glow: 86,
                    motion: 'Stargaze',
                    mountains: 95,
                    palette: 'Toxic Rain',
                    scene: 'Aurora Peak',
                    speed: 3,
                    sunSize: 44,
                },
                description:
                    'Acid-green curtains ripple over a dead peak. The whole sky evolves through impossible hues as radioactive stars drift sideways.',
                name: 'Aurora Borealis',
            },
            {
                controls: {
                    colorMode: 'Evolve',
                    cycleSpeed: 55,
                    glow: 78,
                    motion: 'Serpentine',
                    mountains: 60,
                    palette: 'Sunset Strip',
                    scene: 'Open Road',
                    speed: 5,
                    sunSize: 68,
                },
                description:
                    'Sunset Boulevard melts into amber neon. The whole palette evolves through golden hour as the road writes cursive across the desert.',
                name: 'Sunset Strip',
            },
        ],
    },
)
