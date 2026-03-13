import { effect, num, combo } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Ember Glow', shader, {
    speed:        num('Speed', [1, 10], 5, { group: 'Motion' }),
    flowSpread:   num('Flow Spread', [0, 100], 62, { group: 'Motion' }),
    scene:        combo('Scene', ['Crosswind', 'Updraft', 'Vortex'], { group: 'Scene' }),
    emberDensity: num('Ember Density', [0, 100], 58, { group: 'Scene' }),
    intensity:    num('Intensity', [0, 100], 74, { group: 'Color' }),
    glow:         num('Glow', [0, 100], 68, { group: 'Color' }),
    palette:      combo('Palette', ['Ash Bloom', 'Forge', 'Poison', 'SilkCircuit', 'Toxic Rust'], { group: 'Color' }),
}, {
    description: 'Crisp ember flecks in directional poison-forge flow with selectable scene behavior',
    presets: [
        {
            name: 'Foundry Floor',
            description: 'Sparks cascading off an anvil in a blacksmith\'s den — white-hot flecks riding convection currents upward',
            controls: {
                speed: 6,
                intensity: 90,
                emberDensity: 85,
                flowSpread: 40,
                glow: 80,
                palette: 'Forge',
                scene: 'Updraft',
            },
        },
        {
            name: 'Toxic Spore',
            description: 'Radioactive mycelium releasing glowing spores into a dead wind — slow, alien, unsettling',
            controls: {
                speed: 3,
                intensity: 60,
                emberDensity: 70,
                flowSpread: 90,
                glow: 45,
                palette: 'Poison',
                scene: 'Crosswind',
            },
        },
        {
            name: 'Circuit Meltdown',
            description: 'Overclocked silicon shedding plasma — electric fragments spiraling through a failing motherboard',
            controls: {
                speed: 9,
                intensity: 100,
                emberDensity: 95,
                flowSpread: 75,
                glow: 95,
                palette: 'SilkCircuit',
                scene: 'Vortex',
            },
        },
        {
            name: 'Ash Meditation',
            description: 'Incense embers floating in a still room — barely-there particles drifting with infinite patience',
            controls: {
                speed: 1,
                intensity: 35,
                emberDensity: 25,
                flowSpread: 20,
                glow: 55,
                palette: 'Ash Bloom',
                scene: 'Updraft',
            },
        },
        {
            name: 'Corroded Sun',
            description: 'A dying star venting rust-colored plasma through cracks in its own surface — apocalyptic and beautiful',
            controls: {
                speed: 5,
                intensity: 82,
                emberDensity: 65,
                flowSpread: 55,
                glow: 72,
                palette: 'Toxic Rust',
                scene: 'Vortex',
            },
        },
    ],
})
