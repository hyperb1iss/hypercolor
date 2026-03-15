import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Spectral Fire',
    shader,
    {
        background: combo('Background', ['Void', 'Smoke', 'Midnight', 'Crimson', 'Forest', 'Ember Glow'], {
            group: 'Color',
        }),
        emberAmount: num('Ember Amount', [0, 100], 65, { group: 'Scene' }),
        flameHeight: num('Flame Height', [20, 100], 82, { group: 'Scene' }),
        intensity: num('Intensity', [20, 100], 84, { group: 'Color' }),
        palette: combo(
            'Palette',
            ['Ember', 'Forge', 'Copper', 'Potassium', 'Strontium', 'Barium', 'Cesium', 'Sulfur'],
            { group: 'Color' },
        ),
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
                    background: 'Void',
                    emberAmount: 70,
                    flameHeight: 72,
                    intensity: 68,
                    palette: 'Ember',
                    scene: 'Classic',
                    speed: 4,
                    turbulence: 35,
                },
                description:
                    'A perfect winter fireplace — steady, warm, crackling with occasional pops sending sparks into darkness',
                name: 'Hearthstone',
            },
            {
                controls: {
                    background: 'Crimson',
                    emberAmount: 85,
                    flameHeight: 95,
                    intensity: 100,
                    palette: 'Forge',
                    scene: 'Inferno',
                    speed: 3,
                    turbulence: 40,
                },
                description:
                    'Magma churns in the throat of an active volcano — dense, slow, suffocating heat with white-hot cores',
                name: 'Volcanic Caldera',
            },
            {
                controls: {
                    background: 'Void',
                    emberAmount: 55,
                    flameHeight: 78,
                    intensity: 78,
                    palette: 'Copper',
                    scene: 'Torch',
                    speed: 5,
                    turbulence: 45,
                },
                description:
                    'Copper salts crackle in a crucible — blue-green chemical flames dance with teal precision',
                name: "Alchemist's Flask",
            },
            {
                controls: {
                    background: 'Midnight',
                    emberAmount: 50,
                    flameHeight: 76,
                    intensity: 75,
                    palette: 'Potassium',
                    scene: 'Classic',
                    speed: 5,
                    turbulence: 55,
                },
                description:
                    'Violet spectral flames lick through the bones of a cursed forest — cold fire that consumes nothing',
                name: 'Witch Light',
            },
            {
                controls: {
                    background: 'Crimson',
                    emberAmount: 80,
                    flameHeight: 88,
                    intensity: 92,
                    palette: 'Strontium',
                    scene: 'Wildfire',
                    speed: 7,
                    turbulence: 80,
                },
                description:
                    'Emergency flares split the dark — crimson and magenta flames claw at the sky with desperate intensity',
                name: 'Strontium Flare',
            },
            {
                controls: {
                    background: 'Forest',
                    emberAmount: 30,
                    flameHeight: 58,
                    intensity: 58,
                    palette: 'Barium',
                    scene: 'Torch',
                    speed: 3,
                    turbulence: 25,
                },
                description:
                    'Ghostly green marsh lights hover over still water — flickering low, hypnotic, beckoning you deeper',
                name: "Will-o'-Wisp",
            },
            {
                controls: {
                    background: 'Midnight',
                    emberAmount: 0,
                    flameHeight: 100,
                    intensity: 100,
                    palette: 'Cesium',
                    scene: 'Inferno',
                    speed: 10,
                    turbulence: 100,
                },
                description:
                    'A dimensional rift tears open and vomits pure blue plasma — towering pillars of chaos with no ash, no mercy',
                name: 'Eldritch Gate',
            },
            {
                controls: {
                    background: 'Smoke',
                    emberAmount: 40,
                    flameHeight: 85,
                    intensity: 92,
                    palette: 'Sulfur',
                    scene: 'Wildfire',
                    speed: 7,
                    turbulence: 90,
                },
                description:
                    'Toxic vents split the earth open — acid-yellow flames dance over a field of black glass',
                name: 'Sulfur Rift',
            },
            {
                controls: {
                    background: 'Ember Glow',
                    emberAmount: 100,
                    flameHeight: 42,
                    intensity: 35,
                    palette: 'Ember',
                    scene: 'Classic',
                    speed: 1,
                    turbulence: 15,
                },
                description:
                    'The dying breath of a great fire — low smoldering ash with faint orange pulses cling to life in the dark',
                name: 'Last Ember',
            },
        ],
    },
)
