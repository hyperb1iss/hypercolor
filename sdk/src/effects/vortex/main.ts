import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Vortex',
    shader,
    {
        arms: num('Arms', [2, 8], 4, { group: 'Geometry' }),
        depth: num('Depth', [0, 100], 50, { group: 'Geometry' }),
        intensity: num('Intensity', [0, 100], 60, { group: 'Energy' }),
        palette: combo(
            'Palette',
            ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'Neon Flux', 'Ocean', 'SilkCircuit', 'Synthwave'],
            { group: 'Color' },
        ),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        turbulence: num('Turbulence', [0, 100], 40, { group: 'Chaos' }),
        twist: num('Twist', [0, 100], 60, { group: 'Motion' }),
    },
    {
        description:
            'Surrender to the spiral — logarithmic arms pull inward with differential rotation as vivid color drifts through the whorl',
        presets: [
            {
                controls: {
                    arms: 6,
                    depth: 90,
                    intensity: 85,
                    palette: 'Ocean',
                    speed: 8,
                    turbulence: 70,
                    twist: 95,
                },
                description:
                    'The abyssal maw opens — six churning arms of deep ocean plasma drag everything into a howling singularity of bioluminescent chaos',
                name: 'Charybdis',
            },
            {
                controls: {
                    arms: 3,
                    depth: 65,
                    intensity: 45,
                    palette: 'Aurora',
                    speed: 3,
                    turbulence: 55,
                    twist: 40,
                },
                description:
                    'Northern lights ripped from the magnetosphere and spun into a gravitational lens — aurora ribbons twist through turbulent plasma fields',
                name: 'Aurora Spiral',
            },
            {
                controls: {
                    arms: 5,
                    depth: 100,
                    intensity: 100,
                    palette: 'Fire',
                    speed: 10,
                    turbulence: 85,
                    twist: 100,
                },
                description:
                    'Stellar core collapse at maximum violence — plasma tendrils rip apart as the accretion disk goes supercritical',
                name: 'Supernova Implosion',
            },
            {
                controls: {
                    arms: 4,
                    depth: 55,
                    intensity: 70,
                    palette: 'SilkCircuit',
                    speed: 6,
                    turbulence: 50,
                    twist: 65,
                },
                description:
                    'A living circuit board spiraling through hyperspace — electric filaments arc between the arms as chromatic energy bleeds from the core',
                name: 'SilkCircuit Dynamo',
            },
            {
                controls: {
                    arms: 2,
                    depth: 40,
                    intensity: 55,
                    palette: 'Synthwave',
                    speed: 7,
                    turbulence: 30,
                    twist: 75,
                },
                description:
                    'The neon grid folds into a retro singularity — twin arms of pink and amber plasma spiral inward trailing sparks and chromatic ghosts',
                name: 'Synthwave Drain',
            },
            {
                controls: {
                    arms: 8,
                    depth: 70,
                    intensity: 90,
                    palette: 'Neon Flux',
                    speed: 9,
                    turbulence: 95,
                    twist: 80,
                },
                description:
                    'Pure entropy — eight arms dissolving into turbulent plasma filaments, sparks cascading through a maelstrom of neon static',
                name: 'Hyperstorm',
            },
            {
                controls: {
                    arms: 3,
                    depth: 80,
                    intensity: 50,
                    palette: 'Cyberpunk',
                    speed: 4,
                    turbulence: 60,
                    twist: 50,
                },
                description:
                    'Data streams caught in a gravity well — cyan and magenta tendrils fragment into digital noise as they spiral past the event horizon',
                name: 'Event Horizon',
            },
            {
                controls: {
                    arms: 2,
                    depth: 20,
                    intensity: 30,
                    palette: 'Ice',
                    speed: 2,
                    turbulence: 5,
                    twist: 20,
                },
                description:
                    'Twin glacial arms suspended in crystalline silence — the cold center of a dying star exhaling its last frozen breath',
                name: 'Cryogenic Lullaby',
            },
            {
                controls: {
                    arms: 7,
                    depth: 85,
                    intensity: 75,
                    palette: 'Aurora',
                    speed: 5,
                    turbulence: 40,
                    twist: 90,
                },
                description:
                    'Seven petals of magnetosphere light twist open above the arctic — each arm a curtain of charged particles dancing to solar wind',
                name: 'Polar Bloom',
            },
        ],
    },
)
