import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Vortex', shader, {
    palette: ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'Neon Flux', 'Ocean', 'SilkCircuit', 'Synthwave'],
    speed:   [1, 10, 4],
    arms:    [2, 6, 3],
    twist:   [0, 100, 50],
    depth:   [0, 100, 40],
}, {
    description: 'Mesmerizing logarithmic spiral with differential rotation and vivid color drift',
    presets: [
        {
            name: 'Charybdis',
            description: 'The mythic whirlpool incarnate — six arms of ocean current drag everything toward the abyssal center',
            controls: {
                palette: 'Ocean',
                speed: 7,
                arms: 6,
                twist: 90,
                depth: 85,
            },
        },
        {
            name: 'Aurora Spiral',
            description: 'Northern lights caught in a gravitational lens — gentle aurora ribbons spiral into slow orbital decay',
            controls: {
                palette: 'Aurora',
                speed: 2,
                arms: 3,
                twist: 35,
                depth: 60,
            },
        },
        {
            name: 'Supernova Implosion',
            description: 'A dying star collapses — fire and plasma twist inward at catastrophic speed through the stellar core',
            controls: {
                palette: 'Fire',
                speed: 10,
                arms: 4,
                twist: 100,
                depth: 100,
            },
        },
        {
            name: 'SilkCircuit Dynamo',
            description: 'Electric current visualized as a living spiral — the signature palette hums through rotating magnetic field lines',
            controls: {
                palette: 'SilkCircuit',
                speed: 5,
                arms: 3,
                twist: 55,
                depth: 45,
            },
        },
        {
            name: 'Synthwave Drain',
            description: 'Neon grid reality folds into a retro singularity — pink and cyan arms rotating like a cassette tape rewinding the universe',
            controls: {
                palette: 'Synthwave',
                speed: 6,
                arms: 2,
                twist: 70,
                depth: 30,
            },
        },
    ],
})
