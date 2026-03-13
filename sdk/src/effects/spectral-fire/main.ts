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
        description:
            'Spectral flames lick upward in layered tongues — embers scatter as audio energy lifts the fire into frenzy',
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
                    'Magma churns in the throat of an active volcano — dense, slow, suffocating heat with drifting ash',
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
                    'Spectral green flames lick through the bones of a cursed forest — cold fire that consumes nothing',
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
                description: 'Toxic vents split the earth open — acid-yellow flames dance over a field of black glass',
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
                    'The dying breath of a great fire — low smoldering ash with faint orange pulses cling to life in the dark',
                name: 'Last Ember',
            },
            {
                controls: {
                    emberAmount: 0,
                    flameHeight: 100,
                    intensity: 100,
                    palette: 'Spellfire',
                    scene: 'Inferno',
                    speed: 10,
                    turbulence: 100,
                },
                description:
                    'A dimensional rift tears open and vomits pure spectral plasma — towering green pillars of chaos with no ash, no mercy',
                name: 'Eldritch Gate',
            },
            {
                controls: {
                    emberAmount: 60,
                    flameHeight: 40,
                    intensity: 50,
                    palette: 'Bonfire',
                    scene: 'Wildfire',
                    speed: 8,
                    turbulence: 65,
                },
                description:
                    'Wildfire races across dry prairie at dusk — low frantic flames and kicked-up embers devour the horizon',
                name: 'Prairie Burn',
            },
        ],
    },
)
