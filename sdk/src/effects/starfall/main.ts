import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Starfall', shader, {
    palette:  ['Aurora Rain', 'Celestial', 'Cosmic', 'Ember Fall', 'Frozen Tears', 'Neon Rain'],
    speed:    [1, 10, 5],
    density:  [0, 100, 50],
    trails:   [0, 100, 60],
    sparkle:  [0, 100, 30],
}, {
    description: 'Luminous particles cascading through darkness with glowing comet trails',
    presets: [
        {
            name: 'Meteor Shower',
            description: 'Peak Perseid night in the high desert — blazing streaks tearing across a moonless sky',
            controls: {
                palette: 'Cosmic',
                speed: 8,
                density: 85,
                trails: 90,
                sparkle: 60,
            },
        },
        {
            name: 'Frozen Requiem',
            description: 'Ice crystals falling through the stratosphere — impossibly slow, catching starlight as they descend',
            controls: {
                palette: 'Frozen Tears',
                speed: 2,
                density: 40,
                trails: 75,
                sparkle: 90,
            },
        },
        {
            name: 'Neon Monsoon',
            description: 'Rain on a Tokyo alley at 3AM — every droplet carrying the glow of a thousand signs',
            controls: {
                palette: 'Neon Rain',
                speed: 9,
                density: 100,
                trails: 45,
                sparkle: 20,
            },
        },
        {
            name: 'Celestial Procession',
            description: 'Slow-moving constellation fragments crossing the meridian — stately, ancient, unhurried',
            controls: {
                palette: 'Celestial',
                speed: 3,
                density: 25,
                trails: 95,
                sparkle: 70,
            },
        },
        {
            name: 'Ember Descent',
            description: 'Campfire sparks lifting into a cold mountain night, then falling back to earth as dying light',
            controls: {
                palette: 'Ember Fall',
                speed: 5,
                density: 60,
                trails: 55,
                sparkle: 40,
            },
        },
    ],
})
