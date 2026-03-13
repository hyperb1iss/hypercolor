import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Vortex',
    shader,
    {
        arms: num('Arms', [2, 6], 3, { group: 'Geometry' }),
        depth: num('Depth', [0, 100], 40, { group: 'Geometry' }),
        palette: combo(
            'Palette',
            ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'Neon Flux', 'Ocean', 'SilkCircuit', 'Synthwave'],
            {
                group: 'Color',
            },
        ),
        speed: num('Speed', [1, 10], 4, { group: 'Motion' }),
        twist: num('Twist', [0, 100], 50, { group: 'Motion' }),
    },
    {
        description: 'Mesmerizing logarithmic spiral with differential rotation and vivid color drift',
        presets: [
            {
                controls: {
                    arms: 6,
                    depth: 85,
                    palette: 'Ocean',
                    speed: 7,
                    twist: 90,
                },
                description:
                    'The mythic whirlpool incarnate — six arms of ocean current drag everything toward the abyssal center',
                name: 'Charybdis',
            },
            {
                controls: {
                    arms: 3,
                    depth: 60,
                    palette: 'Aurora',
                    speed: 2,
                    twist: 35,
                },
                description:
                    'Northern lights caught in a gravitational lens — gentle aurora ribbons spiral into slow orbital decay',
                name: 'Aurora Spiral',
            },
            {
                controls: {
                    arms: 4,
                    depth: 100,
                    palette: 'Fire',
                    speed: 10,
                    twist: 100,
                },
                description:
                    'A dying star collapses — fire and plasma twist inward at catastrophic speed through the stellar core',
                name: 'Supernova Implosion',
            },
            {
                controls: {
                    arms: 3,
                    depth: 45,
                    palette: 'SilkCircuit',
                    speed: 5,
                    twist: 55,
                },
                description:
                    'Electric current visualized as a living spiral — the signature palette hums through rotating magnetic field lines',
                name: 'SilkCircuit Dynamo',
            },
            {
                controls: {
                    arms: 2,
                    depth: 30,
                    palette: 'Synthwave',
                    speed: 6,
                    twist: 70,
                },
                description:
                    'Neon grid reality folds into a retro singularity — pink and cyan arms rotating like a cassette tape rewinding the universe',
                name: 'Synthwave Drain',
            },
        ],
    },
)
