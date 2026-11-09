import { describe, expect, test } from 'bun:test'

import { normalizeVendor, parseToml, type VendorEntry } from './gen-compat'

const VENDOR = `[vendor]
name = "Fixture"
vid = [0x1209]
`

function vendor(devices: string): VendorEntry {
    return normalizeVendor('fixture.toml', parseToml(`${VENDOR}\n${devices}`, 'fixture.toml'))
}

function device(fields: string): string {
    return `[[devices]]
pid = 0x0001
name = "Fixture Strip"
status = "supported"
${fields}
`
}

describe('device verification', () => {
    test('defaults to unverified', () => {
        const entry = vendor(device('driver = "fixture"'))

        expect(entry.devices[0].verification).toBe('unverified')
        expect(entry.supportedByVerification).toEqual({ hardware: 0, community: 0, unverified: 1 })
    })

    test('counts verified supported devices by tier', () => {
        const entry = vendor(
            device('verification = "hardware"\ndriver = "fixture"') +
                device('verification = "community"\ndriver = "fixture"'),
        )

        expect(entry.supportedByVerification).toEqual({ hardware: 1, community: 1, unverified: 0 })
        expect(entry.drivers).toEqual(['fixture'])
    })

    test('rejects an unknown tier', () => {
        expect(() => vendor(device('verification = "maybe"\ndriver = "fixture"'))).toThrow(
            'unknown verification "maybe"',
        )
    })

    test('rejects a verified tier without a driver', () => {
        expect(() => vendor(device('verification = "hardware"'))).toThrow('needs a "driver"')
    })
})

describe('driver names', () => {
    test.each([
        ['an empty', 'driver = ""'],
        ['a whitespace-only', 'driver = "   "'],
    ])('rejects %s driver on a verified row', (_label, driverLine) => {
        expect(() => vendor(device(`verification = "hardware"\n${driverLine}`))).toThrow(
            '"driver" is empty',
        )
    })

    test('rejects an empty driver on an unverified row too', () => {
        expect(() => vendor(device('driver = ""'))).toThrow('"driver" is empty')
    })

    test('rejects a driver padded with whitespace', () => {
        expect(() => vendor(device('verification = "hardware"\ndriver = " fixture "'))).toThrow(
            'surrounding whitespace',
        )
    })

    test('rejects a non-string driver', () => {
        expect(() => vendor(device('driver = 7'))).toThrow('expected string for "driver"')
    })

    test('accepts a row with no driver key', () => {
        const entry = vendor(device('').replace('status = "supported"', 'status = "researched"'))

        expect(entry.devices[0].driver).toBeNull()
        expect(entry.devices[0].verification).toBe('unverified')
    })
})
