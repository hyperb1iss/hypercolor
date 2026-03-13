import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Spectral Fire',
    shader,
    {
        emberAmount: num('Ember Amount', [0, 100], 60, { group: 'Scene' }),
        flameHeight: num('Flame Height', [20, 100], 78, { group: 'Scene' }),
        intensity: num('Intensity', [20, 100], 84, { group: 'Color' }),
        palette: combo('Palette', ['Ashfall', 'Bonfire', 'Forge', 'Spellfire', 'Sulfur'], { group: 'Color' }),
        scene: combo('Scene', ['Classic', 'Inferno', 'Torch', 'Wildfire'], { group: 'Scene' }),
        speed: num('Speed', [1, 10], 6, { group: 'Motion' }),
        turbulence: num('Turbulence', [0, 100], 62, { group: 'Motion' }),
    },
    {
        audio: true,
        description: 'Layered fire tongues with embers and optional audio lift',
        presets: [
            {
                controls: {
                    emberAmount: 85,
                    flameHeight: 95,
                    intensity: 100,
                    palette: 'Forge',
                    scene: 'Inferno',
                    speed: 3,
                    turbulence: 40,
                },
                description:
                    'Magma churning in the throat of an active volcano — dense, slow, suffocating heat with drifting ash',
                name: 'Volcanic Caldera',
            },
            {
                controls: {
                    emberAmount: 45,
                    flameHeight: 60,
                    intensity: 70,
                    palette: 'Spellfire',
                    scene: 'Torch',
                    speed: 5,
                    turbulence: 75,
                },
                description:
                    'Spectral green flames licking through the bones of a cursed forest — cold fire that consumes nothing',
                name: 'Witch Light',
            },
            {
                controls: {
                    emberAmount: 70,
                    flameHeight: 55,
                    intensity: 65,
                    palette: 'Bonfire',
                    scene: 'Classic',
                    speed: 4,
                    turbulence: 30,
                },
                description:
                    'A perfect winter fireplace — steady, warm, crackling with occasional pops sending sparks into darkness',
                name: 'Hearthstone',
            },
            {
                controls: {
                    emberAmount: 35,
                    flameHeight: 80,
                    intensity: 92,
                    palette: 'Sulfur',
                    scene: 'Wildfire',
                    speed: 7,
                    turbulence: 90,
                },
                description:
                    'Toxic vents splitting the earth open — acid-yellow flames dancing over a field of black glass',
                name: 'Sulfur Rift',
            },
            {
                controls: {
                    emberAmount: 100,
                    flameHeight: 25,
                    intensity: 30,
                    palette: 'Ashfall',
                    scene: 'Classic',
                    speed: 1,
                    turbulence: 15,
                },
                description:
                    'The dying breath of a great fire — low smoldering ash with faint orange pulses barely clinging to life',
                name: 'Last Ember',
            },
        ],
    },
)
