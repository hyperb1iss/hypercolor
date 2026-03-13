import { effect, num, combo } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Spectral Fire', shader, {
    speed:       num('Speed', [1, 10], 6, { group: 'Motion' }),
    turbulence:  num('Turbulence', [0, 100], 62, { group: 'Motion' }),
    flameHeight: num('Flame Height', [20, 100], 78, { group: 'Scene' }),
    emberAmount: num('Ember Amount', [0, 100], 60, { group: 'Scene' }),
    scene:       combo('Scene', ['Classic', 'Inferno', 'Torch', 'Wildfire'], { group: 'Scene' }),
    intensity:   num('Intensity', [20, 100], 84, { group: 'Color' }),
    palette:     combo('Palette', ['Ashfall', 'Bonfire', 'Forge', 'Spellfire', 'Sulfur'], { group: 'Color' }),
}, {
    description: 'Layered fire tongues with embers and optional audio lift',
    audio: true,
    presets: [
        {
            name: 'Volcanic Caldera',
            description: 'Magma churning in the throat of an active volcano — dense, slow, suffocating heat with drifting ash',
            controls: {
                speed: 3,
                flameHeight: 95,
                turbulence: 40,
                intensity: 100,
                palette: 'Forge',
                emberAmount: 85,
                scene: 'Inferno',
            },
        },
        {
            name: 'Witch Light',
            description: 'Spectral green flames licking through the bones of a cursed forest — cold fire that consumes nothing',
            controls: {
                speed: 5,
                flameHeight: 60,
                turbulence: 75,
                intensity: 70,
                palette: 'Spellfire',
                emberAmount: 45,
                scene: 'Torch',
            },
        },
        {
            name: 'Hearthstone',
            description: 'A perfect winter fireplace — steady, warm, crackling with occasional pops sending sparks into darkness',
            controls: {
                speed: 4,
                flameHeight: 55,
                turbulence: 30,
                intensity: 65,
                palette: 'Bonfire',
                emberAmount: 70,
                scene: 'Classic',
            },
        },
        {
            name: 'Sulfur Rift',
            description: 'Toxic vents splitting the earth open — acid-yellow flames dancing over a field of black glass',
            controls: {
                speed: 7,
                flameHeight: 80,
                turbulence: 90,
                intensity: 92,
                palette: 'Sulfur',
                emberAmount: 35,
                scene: 'Wildfire',
            },
        },
        {
            name: 'Last Ember',
            description: 'The dying breath of a great fire — low smoldering ash with faint orange pulses barely clinging to life',
            controls: {
                speed: 1,
                flameHeight: 25,
                turbulence: 15,
                intensity: 30,
                palette: 'Ashfall',
                emberAmount: 100,
                scene: 'Classic',
            },
        },
    ],
})
