import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Ember Glow',
    shader,
    {
        emberDensity: num('Ember Density', [0, 100], 58, { group: 'Scene' }),
        flowSpread: num('Flow Spread', [0, 100], 62, { group: 'Motion' }),
        glow: num('Glow', [0, 100], 68, { group: 'Color' }),
        intensity: num('Intensity', [0, 100], 74, { group: 'Color' }),
        palette: combo('Palette', ['Ash Bloom', 'Forge', 'Poison', 'SilkCircuit', 'Toxic Rust'], { group: 'Color' }),
        scene: combo('Scene', ['Crosswind', 'Updraft', 'Vortex'], { group: 'Scene' }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
    },
    {
        description: 'Crisp ember flecks in directional poison-forge flow with selectable scene behavior',
        presets: [
            {
                controls: {
                    emberDensity: 85,
                    flowSpread: 40,
                    glow: 80,
                    intensity: 90,
                    palette: 'Forge',
                    scene: 'Updraft',
                    speed: 6,
                },
                description:
                    "Sparks cascading off an anvil in a blacksmith's den — white-hot flecks riding convection currents upward",
                name: 'Foundry Floor',
            },
            {
                controls: {
                    emberDensity: 70,
                    flowSpread: 90,
                    glow: 45,
                    intensity: 60,
                    palette: 'Poison',
                    scene: 'Crosswind',
                    speed: 3,
                },
                description: 'Radioactive mycelium releasing glowing spores into a dead wind — slow, alien, unsettling',
                name: 'Toxic Spore',
            },
            {
                controls: {
                    emberDensity: 95,
                    flowSpread: 75,
                    glow: 95,
                    intensity: 100,
                    palette: 'SilkCircuit',
                    scene: 'Vortex',
                    speed: 9,
                },
                description:
                    'Overclocked silicon shedding plasma — electric fragments spiraling through a failing motherboard',
                name: 'Circuit Meltdown',
            },
            {
                controls: {
                    emberDensity: 25,
                    flowSpread: 20,
                    glow: 55,
                    intensity: 35,
                    palette: 'Ash Bloom',
                    scene: 'Updraft',
                    speed: 1,
                },
                description:
                    'Incense embers floating in a still room — barely-there particles drifting with infinite patience',
                name: 'Ash Meditation',
            },
            {
                controls: {
                    emberDensity: 65,
                    flowSpread: 55,
                    glow: 72,
                    intensity: 82,
                    palette: 'Toxic Rust',
                    scene: 'Vortex',
                    speed: 5,
                },
                description:
                    'A dying star venting rust-colored plasma through cracks in its own surface — apocalyptic and beautiful',
                name: 'Corroded Sun',
            },
        ],
    },
)
