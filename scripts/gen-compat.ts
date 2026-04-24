#!/usr/bin/env bun
/**
 * Compatibility matrix generator.
 *
 * Parses every TOML in data/drivers/vendors/ and emits three outputs:
 *   data/compat/compatibility.json       machine-readable, stable schema, consumed by the site and tooling
 *   docs/content/hardware/compatibility.md   Zola-rendered per-vendor page under docs/
 *   data/compat/README-hardware.md       summary snippet injected into README.md between BEGIN/END markers
 *
 * Usage:
 *   bun scripts/gen-compat.ts             regenerate all outputs
 *   bun scripts/gen-compat.ts --check     fail if any output would differ from what's on disk
 *
 * The script is intentionally dep-free. The vendor TOMLs follow a narrow, regular
 * schema (see data/drivers/README.md), so a small inline parser is simpler than
 * pulling in a package and cheaper than keeping a lockfile in sync.
 */

import { readFileSync, readdirSync, writeFileSync, mkdirSync, existsSync } from 'node:fs'
import { basename, dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

// ─── Paths ──────────────────────────────────────────────────────────────

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const REPO_ROOT = resolve(SCRIPT_DIR, '..')
const VENDORS_DIR = join(REPO_ROOT, 'data', 'drivers', 'vendors')
const COMPAT_DIR = join(REPO_ROOT, 'data', 'compat')
const HARDWARE_MD = join(REPO_ROOT, 'docs', 'content', 'hardware', 'compatibility.md')
const COMPAT_JSON = join(COMPAT_DIR, 'compatibility.json')
const COMPAT_README = join(COMPAT_DIR, 'README-hardware.md')
const README_PATH = join(REPO_ROOT, 'README.md')
const BEGIN_MARKER = '<!-- BEGIN COMPAT -->'
const END_MARKER = '<!-- END COMPAT -->'

// ─── Types ──────────────────────────────────────────────────────────────

type Status = 'supported' | 'in_progress' | 'blocked' | 'planned' | 'researched' | 'known'

const STATUS_ORDER: Status[] = ['supported', 'in_progress', 'planned', 'researched', 'known', 'blocked']

const STATUS_LABELS: Record<Status, string> = {
    supported: 'Supported',
    in_progress: 'In progress',
    planned: 'Planned',
    researched: 'Researched',
    known: 'Known',
    blocked: 'Blocked',
}

const TYPE_LABELS: Record<string, string> = {
    keyboard: 'Keyboard',
    mouse: 'Mouse',
    mousepad: 'Mousepad',
    headset: 'Headset',
    microphone: 'Microphone',
    speakers: 'Speakers',
    aio: 'AIO cooler',
    fan_controller: 'Fan controller',
    argb_controller: 'ARGB controller',
    gpu: 'GPU',
    motherboard: 'Motherboard',
    ram: 'RAM',
    monitor: 'Monitor',
    lcd: 'LCD module',
    lightbar: 'Lightbar',
    case: 'Case',
    desk: 'Desk accessory',
    strip: 'LED strip',
    other: 'Other',
}

const TRANSPORT_LABELS: Record<string, string> = {
    usb_hid: 'USB HID',
    usb_hid_raw: 'USB HID (raw)',
    usb_control: 'USB control transfer',
    usb_bulk: 'USB bulk transfer',
    usb_serial: 'USB serial',
    usb_vendor: 'USB vendor',
    usb_midi: 'USB MIDI',
    i2c_smbus: 'I²C / SMBus',
    network_http: 'HTTP',
    network_udp: 'UDP',
    network_mdns: 'mDNS',
}

interface DeviceEntry {
    pid: number | null
    pidHex: string | null
    name: string
    type: string | null
    status: Status
    driver: string | null
    transport: string | null
    leds: number | null
    notes: string | null
}

interface VendorEntry {
    slug: string
    name: string
    website: string | null
    vids: number[]
    vidsHex: string[]
    notes: string | null
    devices: DeviceEntry[]
    counts: Record<Status, number>
    totalDevices: number
    drivers: string[]
    transports: string[]
    deviceTypes: string[]
    sourceFile: string
}

interface Report {
    schemaVersion: 1
    generatedAt: string
    generatorPath: string
    totals: {
        vendors: number
        devices: number
        byStatus: Record<Status, number>
        drivers: string[]
    }
    vendors: VendorEntry[]
}

// ─── Tiny TOML parser ───────────────────────────────────────────────────
//
// Handles the subset actually used by data/drivers/vendors/*.toml:
//   [vendor]   single-table header
//   [[devices]]   array-of-tables header
//   key = value   scalar assignments
//   Values: quoted string, integer (decimal or 0x hex), boolean, inline array
//   # comments on their own line or trailing a non-string line
//
// If the schema grows past this subset, upgrade to a real TOML library rather
// than extending this parser.

interface ParsedToml {
    vendor: Record<string, unknown>
    devices: Record<string, unknown>[]
}

const VENDOR_KEYS = new Set(['name', 'vid', 'website', 'notes'])
const DEVICE_KEYS = new Set(['pid', 'name', 'type', 'status', 'driver', 'transport', 'leds', 'notes'])

function parseToml(src: string, file: string): ParsedToml {
    const out: ParsedToml = { vendor: {}, devices: [] }
    let target: Record<string, unknown> | null = null
    let scope: 'vendor' | 'devices' | null = null
    const lines = src.split(/\r?\n/)

    for (let i = 0; i < lines.length; i++) {
        const raw = lines[i]
        const line = stripTrailingComment(raw).trim()
        if (!line) continue
        if (line.startsWith('#')) continue

        if (line === '[[devices]]') {
            const device: Record<string, unknown> = {}
            out.devices.push(device)
            target = device
            scope = 'devices'
            continue
        }
        if (line === '[vendor]') {
            target = out.vendor
            scope = 'vendor'
            continue
        }
        if (line.startsWith('[')) {
            throw new Error(`${file}:${i + 1}: unsupported section header "${line}"`)
        }

        const eq = line.indexOf('=')
        if (eq < 0) {
            throw new Error(`${file}:${i + 1}: expected "key = value", got "${line}"`)
        }
        if (!target || !scope) {
            throw new Error(`${file}:${i + 1}: assignment outside any section`)
        }

        const key = line.slice(0, eq).trim()
        const allowed = scope === 'vendor' ? VENDOR_KEYS : DEVICE_KEYS
        if (!allowed.has(key)) {
            throw new Error(
                `${file}:${i + 1}: unknown ${scope} key "${key}". Expected one of: ${Array.from(allowed).join(', ')}`,
            )
        }
        if (key in target) {
            throw new Error(`${file}:${i + 1}: duplicate key "${key}" in ${scope} section`)
        }

        const rawValue = line.slice(eq + 1).trim()
        target[key] = parseValue(rawValue, file, i + 1)
    }

    return out
}

function stripTrailingComment(line: string): string {
    // Walk the line tracking whether we're inside a double-quoted string; drop
    // everything from the first unquoted '#' onward. The inString toggle uses
    // a backslash-run count so escaped quotes inside strings don't confuse us.
    let inString = false
    for (let i = 0; i < line.length; i++) {
        const ch = line[i]
        if (ch === '"') {
            let bs = 0
            for (let j = i - 1; j >= 0 && line[j] === '\\'; j--) bs++
            if (bs % 2 === 0) inString = !inString
        } else if (ch === '#' && !inString) {
            return line.slice(0, i)
        }
    }
    return line
}

function parseValue(raw: string, file: string, lineNo: number): unknown {
    if (raw.length === 0) {
        throw new Error(`${file}:${lineNo}: empty value`)
    }
    if (raw.startsWith('"')) {
        const { value, end } = parseString(raw, file, lineNo)
        const trailing = raw.slice(end + 1).trim()
        if (trailing.length > 0) {
            throw new Error(`${file}:${lineNo}: trailing garbage after string: "${trailing}"`)
        }
        return value
    }
    if (raw.startsWith('[')) {
        const end = raw.lastIndexOf(']')
        if (end < 0) throw new Error(`${file}:${lineNo}: unterminated array: ${raw}`)
        const trailing = raw.slice(end + 1).trim()
        if (trailing.length > 0) {
            throw new Error(`${file}:${lineNo}: trailing garbage after array: "${trailing}"`)
        }
        const inner = raw.slice(1, end).trim()
        if (!inner) return []
        return splitTopLevel(inner).map((part) => parseValue(part.trim(), file, lineNo))
    }
    if (raw === 'true') return true
    if (raw === 'false') return false
    if (/^-?0x[0-9a-fA-F]+$/.test(raw)) return Number.parseInt(raw, 16)
    if (/^-?\d+$/.test(raw)) return Number.parseInt(raw, 10)
    if (/^-?\d+\.\d+$/.test(raw)) return Number.parseFloat(raw)
    throw new Error(`${file}:${lineNo}: unrecognized value "${raw}"`)
}

// Parse a double-quoted TOML basic string. Handles the escape sequences TOML
// actually specifies for basic strings. Returns { value, end } where `end` is
// the index of the closing quote within `raw`.
function parseString(raw: string, file: string, lineNo: number): { value: string; end: number } {
    const buf: string[] = []
    let i = 1
    while (i < raw.length) {
        const ch = raw[i]
        if (ch === '\\') {
            const next = raw[i + 1]
            switch (next) {
                case '"':
                    buf.push('"')
                    i += 2
                    continue
                case '\\':
                    buf.push('\\')
                    i += 2
                    continue
                case 'n':
                    buf.push('\n')
                    i += 2
                    continue
                case 't':
                    buf.push('\t')
                    i += 2
                    continue
                case 'r':
                    buf.push('\r')
                    i += 2
                    continue
                case 'b':
                    buf.push('\b')
                    i += 2
                    continue
                case 'f':
                    buf.push('\f')
                    i += 2
                    continue
                case '/':
                    buf.push('/')
                    i += 2
                    continue
                default:
                    throw new Error(
                        `${file}:${lineNo}: unsupported escape "\\${next ?? ''}" in string. ` +
                            'Use only "\\n", "\\t", "\\r", "\\b", "\\f", "\\/", "\\"" or "\\\\".',
                    )
            }
        }
        if (ch === '"') {
            return { value: buf.join(''), end: i }
        }
        buf.push(ch)
        i++
    }
    throw new Error(`${file}:${lineNo}: unterminated string`)
}

function splitTopLevel(src: string): string[] {
    const parts: string[] = []
    let depth = 0
    let inString = false
    let start = 0
    for (let i = 0; i < src.length; i++) {
        const ch = src[i]
        if (ch === '"') {
            // Count consecutive backslashes behind this quote; an odd count
            // means the quote is escaped, an even count (including zero) means
            // it's a real string delimiter.
            let bs = 0
            for (let j = i - 1; j >= 0 && src[j] === '\\'; j--) bs++
            if (bs % 2 === 0) inString = !inString
        } else if (!inString) {
            if (ch === '[') depth++
            else if (ch === ']') depth--
            else if (ch === ',' && depth === 0) {
                parts.push(src.slice(start, i))
                start = i + 1
            }
        }
    }
    parts.push(src.slice(start))
    return parts.filter((p) => p.trim().length > 0)
}

// ─── Loader ─────────────────────────────────────────────────────────────

function loadVendors(): VendorEntry[] {
    if (!existsSync(VENDORS_DIR)) {
        throw new Error(`vendor directory not found: ${VENDORS_DIR}`)
    }
    const files = readdirSync(VENDORS_DIR)
        .filter((f) => f.endsWith('.toml'))
        .sort()

    const vendors: VendorEntry[] = []
    for (const file of files) {
        const full = join(VENDORS_DIR, file)
        const raw = readFileSync(full, 'utf8')
        const parsed = parseToml(raw, file)
        vendors.push(normalizeVendor(file, parsed))
    }

    vendors.sort((a, b) => a.name.localeCompare(b.name, 'en', { sensitivity: 'base' }))
    return vendors
}

function normalizeVendor(file: string, src: ParsedToml): VendorEntry {
    const vendor = src.vendor
    const slug = basename(file, '.toml')
    const name = requireString(vendor, 'name', file)
    const website = typeof vendor.website === 'string' ? vendor.website : null
    const notes = typeof vendor.notes === 'string' && vendor.notes.length > 0 ? vendor.notes : null
    const vids = coerceNumberArray(vendor.vid, file, 'vendor.vid')

    const counts = emptyCounts()
    const drivers = new Set<string>()
    const transports = new Set<string>()
    const deviceTypes = new Set<string>()

    const devices: DeviceEntry[] = []
    for (let i = 0; i < src.devices.length; i++) {
        const d = src.devices[i]
        const status = requireStatus(d, `${file}[${i}]`)
        const name = requireString(d, 'name', `${file}[${i}]`)
        const pid = typeof d.pid === 'number' ? d.pid : null
        const type = typeof d.type === 'string' ? d.type : null
        const driver = typeof d.driver === 'string' ? d.driver : null
        const transport = typeof d.transport === 'string' ? d.transport : null
        const leds = typeof d.leds === 'number' ? d.leds : null
        const notes = typeof d.notes === 'string' && d.notes.length > 0 ? d.notes : null

        counts[status] += 1
        if (driver) drivers.add(driver)
        if (transport) transports.add(transport)
        if (type) deviceTypes.add(type)

        devices.push({
            pid,
            pidHex: pid !== null ? formatHex(pid, 4) : null,
            name,
            type,
            status,
            driver,
            transport,
            leds,
            notes,
        })
    }

    // Sort devices: status order, then name within status.
    devices.sort((a, b) => {
        const sa = STATUS_ORDER.indexOf(a.status)
        const sb = STATUS_ORDER.indexOf(b.status)
        if (sa !== sb) return sa - sb
        return a.name.localeCompare(b.name, 'en', { sensitivity: 'base' })
    })

    return {
        slug,
        name,
        website,
        vids,
        vidsHex: vids.map((v) => formatHex(v, 4)),
        notes,
        devices,
        counts,
        totalDevices: devices.length,
        drivers: Array.from(drivers).sort(),
        transports: Array.from(transports).sort(),
        deviceTypes: Array.from(deviceTypes).sort(),
        sourceFile: `data/drivers/vendors/${file}`,
    }
}

function emptyCounts(): Record<Status, number> {
    return {
        supported: 0,
        in_progress: 0,
        planned: 0,
        researched: 0,
        known: 0,
        blocked: 0,
    }
}

function requireString(obj: Record<string, unknown>, key: string, where: string): string {
    const v = obj[key]
    if (typeof v !== 'string') {
        throw new Error(`${where}: expected string for "${key}", got ${typeof v}`)
    }
    return v
}

function requireStatus(obj: Record<string, unknown>, where: string): Status {
    const v = obj.status
    if (typeof v !== 'string') {
        throw new Error(`${where}: missing or non-string "status"`)
    }
    if (!STATUS_ORDER.includes(v as Status)) {
        throw new Error(`${where}: unknown status "${v}" (expected one of ${STATUS_ORDER.join(', ')})`)
    }
    return v as Status
}

function coerceNumberArray(v: unknown, file: string, path: string): number[] {
    if (v === undefined || v === null) return []
    if (!Array.isArray(v)) {
        throw new Error(`${file}: expected array for ${path}`)
    }
    return v.map((n, i) => {
        if (typeof n !== 'number' || !Number.isInteger(n)) {
            throw new Error(`${file}: ${path}[${i}] is not an integer`)
        }
        return n
    })
}

function formatHex(value: number, minDigits: number): string {
    return `0x${value.toString(16).toUpperCase().padStart(minDigits, '0')}`
}

// ─── Rollup ─────────────────────────────────────────────────────────────

function buildReport(vendors: VendorEntry[]): Report {
    const byStatus = emptyCounts()
    // Driver families that contribute at least one shipping device. A driver
    // that's implemented but whose devices are all `blocked` (e.g. dygma) does
    // not count toward the "X driver families" surface in marketing copy,
    // because nothing shipping is actually talking to a user's hardware.
    const drivers = new Set<string>()
    let deviceCount = 0
    for (const v of vendors) {
        for (const s of STATUS_ORDER) byStatus[s] += v.counts[s]
        for (const d of v.devices) {
            if (d.status === 'supported' && d.driver) drivers.add(d.driver)
        }
        deviceCount += v.totalDevices
    }

    return {
        schemaVersion: 1,
        generatedAt: new Date().toISOString(),
        generatorPath: 'scripts/gen-compat.ts',
        totals: {
            vendors: vendors.length,
            devices: deviceCount,
            byStatus,
            drivers: Array.from(drivers).sort(),
        },
        vendors,
    }
}

// ─── Formatters ─────────────────────────────────────────────────────────

function renderJson(report: Report): string {
    // Stable output: the generatedAt timestamp makes diffs noisy on every run.
    // Strip it when we're writing to disk so the committed JSON only changes
    // when the underlying data does. CI can still see the diff via `--check`.
    const { generatedAt: _timestamp, ...stable } = report
    return `${JSON.stringify(stable, null, 2)}\n`
}

function renderHardwareMd(report: Report): string {
    const lines: string[] = []
    lines.push('+++')
    lines.push('title = "Compatibility Matrix"')
    lines.push('description = "Every vendor and device tracked in Hypercolor\'s driver database, with shipping status."')
    lines.push('weight = 2')
    lines.push('template = "page.html"')
    lines.push('+++')
    lines.push('')
    lines.push(MARKER_WARN)
    lines.push('')
    lines.push(summaryParagraph(report))
    lines.push('')
    lines.push('## Summary')
    lines.push('')
    lines.push(...summaryTable(report))
    lines.push('')
    lines.push('## By Driver')
    lines.push('')
    lines.push(...byDriverList(report))
    lines.push('')
    lines.push('## Supported Devices')
    lines.push('')
    lines.push(...statusTable(report, 'supported'))
    lines.push('')
    if (report.totals.byStatus.in_progress > 0) {
        lines.push('## In Progress')
        lines.push('')
        lines.push(...statusTable(report, 'in_progress'))
        lines.push('')
    }
    if (report.totals.byStatus.planned > 0) {
        lines.push('## Planned')
        lines.push('')
        lines.push(...statusTable(report, 'planned'))
        lines.push('')
    }
    lines.push('## Researched (Awaiting Implementation)')
    lines.push('')
    lines.push(...statusTable(report, 'researched'))
    lines.push('')
    if (report.totals.byStatus.blocked > 0) {
        lines.push('## Blocked')
        lines.push('')
        lines.push('Hardware where an initial driver exists but the device itself cannot currently be controlled, typically pending a firmware or protocol change outside Hypercolor.')
        lines.push('')
        lines.push(...statusTable(report, 'blocked'))
        lines.push('')
    }
    if (report.totals.byStatus.known > 0) {
        lines.push('## Known (Protocol Research Pending)')
        lines.push('')
        lines.push('Devices present in the database but not yet researched. These are opportunities for contributors to capture USB traces and document protocols.')
        lines.push('')
        lines.push(...statusTable(report, 'known'))
        lines.push('')
    }
    lines.push('## Per-Vendor Details')
    lines.push('')
    for (const vendor of report.vendors) {
        lines.push(...vendorSection(vendor))
        lines.push('')
    }
    return `${lines.join('\n').trimEnd()}\n`
}

function renderReadmeSnippet(report: Report): string {
    const lines: string[] = []
    lines.push(`<!-- GENERATED by scripts/gen-compat.ts. Do not edit inside the BEGIN/END COMPAT block. Run \`just compat\` to refresh. -->`)
    lines.push('')
    lines.push(summaryParagraph(report))
    lines.push('')
    lines.push(...summaryTable(report, { anchorBase: 'docs/content/hardware/compatibility.md' }))
    lines.push('')
    lines.push('New drivers land often. Full matrix: [docs/content/hardware/compatibility.md](docs/content/hardware/compatibility.md). If you own hardware Hypercolor doesn\'t support yet, see [CONTRIBUTING.md](CONTRIBUTING.md).')
    return `${lines.join('\n').trimEnd()}\n`
}

// ─── Section helpers ────────────────────────────────────────────────────

const MARKER_WARN = '<!-- GENERATED by scripts/gen-compat.ts from data/drivers/vendors/*.toml. Do not edit by hand; run `just compat` to regenerate. -->'

function summaryParagraph(report: Report): string {
    const { totals } = report
    const supported = totals.byStatus.supported
    const inFlight = totals.byStatus.in_progress + totals.byStatus.planned
    const researchPool = totals.byStatus.researched + totals.byStatus.known
    const drivers = totals.drivers.length

    const segments = [
        `Hypercolor tracks **${totals.devices} devices** across **${totals.vendors} vendors** in \`data/drivers/vendors/\`.`,
        `**${supported}** ship with a working driver today across **${drivers} driver families**.`,
    ]
    if (inFlight > 0) {
        segments.push(`**${inFlight}** are actively in development.`)
    }
    if (researchPool > 0) {
        segments.push(`**${researchPool} more** are researched or known, awaiting implementation or hardware to test.`)
    }
    return segments.join(' ')
}

interface SummaryTableOptions {
    // When set, vendor links resolve to `<anchorBase>#<slug>` (used in the
    // README snippet to point at the full matrix page). When absent, links
    // are bare `#<slug>` anchors for same-page navigation inside the Zola doc.
    anchorBase?: string
}

function summaryTable(report: Report, opts: SummaryTableOptions = {}): string[] {
    const rows: string[] = []
    rows.push('| Vendor | Supported | In progress | Researched | Blocked | Drivers |')
    rows.push('|---|--:|--:|--:|--:|---|')
    for (const v of report.vendors) {
        const drivers = v.drivers.length > 0 ? v.drivers.map((d) => `\`${d}\``).join(', ') : '—'
        rows.push(
            `| ${vendorLink(v, opts.anchorBase)} | ${fmtCount(v.counts.supported)} | ${fmtCount(v.counts.in_progress + v.counts.planned)} | ${fmtCount(v.counts.researched + v.counts.known)} | ${fmtCount(v.counts.blocked)} | ${drivers} |`,
        )
    }
    return rows
}

function byDriverList(report: Report): string[] {
    const counts = new Map<string, number>()
    for (const v of report.vendors) {
        for (const d of v.devices) {
            if (d.status === 'supported' && d.driver) {
                counts.set(d.driver, (counts.get(d.driver) ?? 0) + 1)
            }
        }
    }
    const items = Array.from(counts.entries()).sort(
        (a, b) => b[1] - a[1] || a[0].localeCompare(b[0]),
    )
    if (items.length === 0) return ['_No shipping drivers yet._']
    return items.map(([driver, count]) => `- **\`${driver}\`** — ${count} supported device${count === 1 ? '' : 's'}`)
}

function statusTable(report: Report, status: Status): string[] {
    const rows: string[] = []
    // PID is vendor-scoped; multi-VID vendors (like QMK, which has nine VIDs,
    // or Lian Li which straddles two) encode the per-device VID inside the
    // `notes` string rather than as a structured field, so we show PID alone
    // and let the vendor section header surface the VID list.
    const showPid = report.vendors.some((v) => v.devices.some((d) => d.status === status && d.pidHex))
    const showLeds = status === 'supported' || status === 'in_progress'
    const showDriver = status === 'supported' || status === 'in_progress' || status === 'blocked'

    const header = ['Vendor', 'Device']
    if (showPid) header.push('PID')
    header.push('Type')
    if (showDriver) header.push('Driver')
    header.push('Transport')
    if (showLeds) header.push('LEDs')
    header.push('Notes')

    rows.push(`| ${header.join(' | ')} |`)
    rows.push(`|${header.map(() => '---').join('|')}|`)

    let any = false
    for (const v of report.vendors) {
        for (const d of v.devices) {
            if (d.status !== status) continue
            any = true
            const cells: string[] = [escape(v.name), escape(d.name)]
            if (showPid) cells.push(d.pidHex ?? '—')
            cells.push(typeLabel(d.type))
            if (showDriver) cells.push(d.driver ? `\`${d.driver}\`` : '—')
            cells.push(transportLabel(d.transport))
            if (showLeds) cells.push(d.leds !== null ? String(d.leds) : '—')
            cells.push(notesCell(d.notes))
            rows.push(`| ${cells.join(' | ')} |`)
        }
    }
    if (!any) rows.push('| — | — | — | — | — |')
    return rows
}

function vendorSection(vendor: VendorEntry): string[] {
    const lines: string[] = []
    const header = `### ${vendor.name} {#${anchor(vendor.slug)}}`
    lines.push(header)
    const meta: string[] = []
    if (vendor.website) meta.push(`[${prettyHost(vendor.website)}](${vendor.website})`)
    if (vendor.vidsHex.length > 0) meta.push(`VID ${vendor.vidsHex.join(', ')}`)
    if (vendor.drivers.length > 0) meta.push(`Driver \`${vendor.drivers.join(', ')}\``)
    if (meta.length > 0) {
        lines.push('')
        lines.push(meta.join(' · '))
    }
    lines.push('')

    const summaryParts: string[] = []
    for (const s of STATUS_ORDER) {
        if (vendor.counts[s] > 0) summaryParts.push(`${vendor.counts[s]} ${STATUS_LABELS[s].toLowerCase()}`)
    }
    lines.push(`**Devices tracked:** ${summaryParts.join(' · ') || 'none'}`)
    lines.push('')

    if (vendor.notes) {
        lines.push(`> ${vendor.notes}`)
        lines.push('')
    }

    for (const status of STATUS_ORDER) {
        const matches = vendor.devices.filter((d) => d.status === status)
        if (matches.length === 0) continue
        lines.push(`#### ${STATUS_LABELS[status]} (${matches.length})`)
        lines.push('')
        lines.push(...vendorDeviceTable(matches, status))
        lines.push('')
    }

    lines.push(`_Source: [\`${vendor.sourceFile}\`](${relativeLink(vendor.sourceFile)})_`)
    return lines
}

function vendorDeviceTable(devices: DeviceEntry[], status: Status): string[] {
    const rows: string[] = []
    const showPid = devices.some((d) => d.pidHex)
    const showDriver = status === 'supported' || status === 'in_progress' || status === 'blocked'
    const showLeds = (status === 'supported' || status === 'in_progress') && devices.some((d) => d.leds !== null)

    const header = ['Device']
    if (showPid) header.push('PID')
    header.push('Type')
    if (showDriver) header.push('Driver')
    header.push('Transport')
    if (showLeds) header.push('LEDs')
    header.push('Notes')

    rows.push(`| ${header.join(' | ')} |`)
    rows.push(`|${header.map(() => '---').join('|')}|`)

    for (const d of devices) {
        const cells: string[] = [escape(d.name)]
        if (showPid) cells.push(d.pidHex ?? '—')
        cells.push(typeLabel(d.type))
        if (showDriver) cells.push(d.driver ? `\`${d.driver}\`` : '—')
        cells.push(transportLabel(d.transport))
        if (showLeds) cells.push(d.leds !== null ? String(d.leds) : '—')
        cells.push(notesCell(d.notes))
        rows.push(`| ${cells.join(' | ')} |`)
    }
    return rows
}

// ─── Small helpers ──────────────────────────────────────────────────────

function vendorLink(v: VendorEntry, anchorBase?: string): string {
    const target = anchorBase ? `${anchorBase}#${anchor(v.slug)}` : `#${anchor(v.slug)}`
    return `[${escape(v.name)}](${target})`
}

function anchor(slug: string): string {
    return slug.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
}

function typeLabel(t: string | null): string {
    if (!t) return '—'
    return TYPE_LABELS[t] ?? t
}

function transportLabel(t: string | null): string {
    if (!t) return '—'
    return TRANSPORT_LABELS[t] ?? t
}

function fmtCount(n: number): string {
    return n === 0 ? '—' : String(n)
}

function notesCell(notes: string | null): string {
    if (!notes) return '—'
    // Escape pipes so the notes don't break the markdown table.
    return notes.replace(/\|/g, '\\|')
}

function escape(s: string): string {
    return s.replace(/\|/g, '\\|')
}

function prettyHost(url: string): string {
    try {
        const u = new URL(url)
        return u.hostname.replace(/^www\./, '')
    } catch {
        return url
    }
}

function relativeLink(pathFromRepo: string): string {
    // docs/content/hardware/compatibility.md → ../../../<path>
    return join('..', '..', '..', pathFromRepo).split('\\').join('/')
}

// ─── Write / check ──────────────────────────────────────────────────────

// Inject the generated summary into README.md between BEGIN/END markers.
// If markers are absent, emit a warning and leave the README alone; this keeps
// the generator usable against a repo where the README hasn't been wired up
// yet, while still giving CI a signal via the warning text.
function injectIntoReadme(snippet: string, check: boolean): { changed: boolean } {
    const rel = relative(REPO_ROOT, README_PATH)
    if (!existsSync(README_PATH)) {
        console.warn(`  ! ${rel} missing; skipping injection`)
        return { changed: false }
    }
    const readme = readFileSync(README_PATH, 'utf8')
    const beginIdx = readme.indexOf(BEGIN_MARKER)
    const endIdx = readme.indexOf(END_MARKER)
    if (beginIdx < 0 || endIdx < 0) {
        console.warn(`  ! ${rel}: BEGIN COMPAT / END COMPAT markers not found; skipping injection`)
        return { changed: false }
    }
    if (endIdx < beginIdx) {
        throw new Error(`${rel}: END COMPAT appears before BEGIN COMPAT`)
    }
    const before = readme.slice(0, beginIdx + BEGIN_MARKER.length)
    const after = readme.slice(endIdx)
    const next = `${before}\n${snippet}${after}`
    if (check) {
        if (next !== readme) {
            console.error(`  ✗ ${rel} (compat block) is out of date`)
            return { changed: true }
        }
        console.log(`  ✓ ${rel} (compat block) is current`)
        return { changed: false }
    }
    if (next === readme) {
        console.log(`  · ${rel} (compat block) unchanged`)
        return { changed: false }
    }
    writeFileSync(README_PATH, next)
    console.log(`  ✓ ${rel} (compat block) updated`)
    return { changed: true }
}

function writeOrCheck(path: string, contents: string, check: boolean): { changed: boolean } {
    const rel = relative(REPO_ROOT, path)
    if (check) {
        const existing = existsSync(path) ? readFileSync(path, 'utf8') : null
        if (existing !== contents) {
            console.error(`  ✗ ${rel} is out of date`)
            return { changed: true }
        }
        console.log(`  ✓ ${rel} is current`)
        return { changed: false }
    }
    mkdirSync(dirname(path), { recursive: true })
    const prev = existsSync(path) ? readFileSync(path, 'utf8') : null
    if (prev === contents) {
        console.log(`  · ${rel} unchanged`)
        return { changed: false }
    }
    writeFileSync(path, contents)
    console.log(`  ✓ ${rel} written`)
    return { changed: true }
}

// ─── Entry point ────────────────────────────────────────────────────────

function main() {
    const args = process.argv.slice(2)
    const check = args.includes('--check')

    const vendors = loadVendors()
    const report = buildReport(vendors)

    const snippet = renderReadmeSnippet(report)
    const outputs: Array<[string, string]> = [
        [COMPAT_JSON, renderJson(report)],
        [HARDWARE_MD, renderHardwareMd(report)],
        [COMPAT_README, snippet],
    ]

    let anyChanged = false
    console.log(check ? '→ Checking compatibility matrix is current…' : '→ Generating compatibility matrix…')
    for (const [path, content] of outputs) {
        const { changed } = writeOrCheck(path, content, check)
        if (changed) anyChanged = true
    }

    const readmeResult = injectIntoReadme(snippet, check)
    if (readmeResult.changed) anyChanged = true

    console.log('')
    const t = report.totals
    console.log(`  ${t.vendors} vendors · ${t.devices} devices · ${t.byStatus.supported} supported · ${t.byStatus.researched + t.byStatus.known} researched/known · ${t.byStatus.blocked} blocked`)
    console.log(`  drivers: ${t.drivers.join(', ')}`)

    if (check && anyChanged) {
        console.error('\n✗ Compatibility matrix is out of date. Run `just compat` to regenerate.')
        process.exit(1)
    }
    if (check) {
        console.log('\n✓ Compatibility matrix is up to date.')
        return
    }
    console.log(anyChanged ? '\n✓ Compatibility matrix regenerated.' : '\n✓ Compatibility matrix already current (no changes).')
}

main()
