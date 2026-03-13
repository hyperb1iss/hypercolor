import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Starfall',
    shader,
    {
        density: num('Density', [0, 100], 50, { group: 'Atmosphere' }),
        palette: combo('Palette', ['Aurora Rain', 'Celestial', 'Cosmic', 'Ember Fall', 'Frozen Tears', 'Neon Rain'], {
            group: 'Color',
        }),
        sparkle: num('Sparkle', [0, 100], 30, { group: 'Color' }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        trails: num('Trails', [0, 100], 60, { group: 'Atmosphere' }),
    },
    {
        description:
            'Luminous particles cascade through infinite black — each one trailing a comet glow as it falls through the dark',
        presets: [
            {
                controls: {
                    density: 85,
                    palette: 'Cosmic',
                    sparkle: 60,
                    speed: 8,
                    trails: 90,
                },
                description: 'Peak Perseid night in the high desert — blazing streaks tear across a moonless sky',
                name: 'Meteor Shower',
            },
            {
                controls: {
                    density: 40,
                    palette: 'Frozen Tears',
                    sparkle: 90,
                    speed: 2,
                    trails: 75,
                },
                description:
                    'Ice crystals fall through the stratosphere — impossibly slow, catching starlight as they descend',
                name: 'Frozen Requiem',
            },
            {
                controls: {
                    density: 100,
                    palette: 'Neon Rain',
                    sparkle: 20,
                    speed: 9,
                    trails: 45,
                },
                description: 'Rain hammers a Tokyo alley at 3AM — every droplet carries the glow of a thousand signs',
                name: 'Neon Monsoon',
            },
            {
                controls: {
                    density: 25,
                    palette: 'Celestial',
                    sparkle: 70,
                    speed: 3,
                    trails: 95,
                },
                description: 'Constellation fragments cross the meridian in silence — stately, ancient, unhurried',
                name: 'Celestial Procession',
            },
            {
                controls: {
                    density: 60,
                    palette: 'Ember Fall',
                    sparkle: 40,
                    speed: 5,
                    trails: 55,
                },
                description: 'Campfire sparks lift into a cold mountain night, then fall back to earth as dying light',
                name: 'Ember Descent',
            },
            {
                controls: {
                    density: 10,
                    palette: 'Aurora Rain',
                    sparkle: 100,
                    speed: 1,
                    trails: 100,
                },
                description:
                    'A single luminous thread unspools from the aurora — one solitary particle descends forever, dragging the entire sky behind it',
                name: 'Last Light of Thule',
            },
            {
                controls: {
                    density: 100,
                    palette: 'Cosmic',
                    sparkle: 0,
                    speed: 10,
                    trails: 0,
                },
                description:
                    'The sky collapses — a hundred thousand particles plummet without trails, pure velocity, a white-noise waterfall of falling stars',
                name: 'Extinction Event',
            },
        ],
    },
)
