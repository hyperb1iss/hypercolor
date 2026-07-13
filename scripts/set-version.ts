#!/usr/bin/env bun
/**
 * set-version.ts — stamp one release version across every version-bearing
 * file in the repository.
 *
 * Usage:
 *   bun scripts/set-version.ts <version>            # stamp
 *   bun scripts/set-version.ts <version> --verify   # assert already stamped
 *
 * Stamped files:
 *   - Cargo.toml                      [workspace.package] version (all crates inherit)
 *   - crates/hypercolor-app/tauri.conf.json
 *   - python/pyproject.toml           (semver prerelease translated to PEP 440)
 *   - packaging/aur/PKGBUILD          (stable releases only; AUR forbids hyphens)
 *   - sdk/packages/core/package.json
 *   - sdk/packages/create-effect/package.json
 *
 * Lockfiles are NOT touched here — the release workflow refreshes them
 * (cargo update --workspace, bun install, uv lock) after stamping.
 */

import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const REPO_ROOT = resolve(import.meta.dirname, '..')

const SEMVER = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z][0-9A-Za-z.-]*))?$/

function fail(message: string): never {
    console.error(`✗ ${message}`)
    process.exit(1)
}

function pep440(version: string): string {
    const match = SEMVER.exec(version)
    if (!match) fail(`not a valid semver version: ${version}`)
    const [, major, minor, patch, pre] = match
    const base = `${major}.${minor}.${patch}`
    if (!pre) return base

    const preMatch = /^(alpha|beta|rc)\.(\d+)$/.exec(pre)
    if (!preMatch) {
        fail(
            `prerelease "${pre}" has no PEP 440 mapping — use alpha.N, beta.N, or rc.N ` +
                'so the Python package version stays valid',
        )
    }
    const kind = { alpha: 'a', beta: 'b', rc: 'rc' }[preMatch[1] as 'alpha' | 'beta' | 'rc']
    return `${base}${kind}${preMatch[2]}`
}

interface Target {
    path: string
    /** Replace the version in the file content; return null when no match. */
    stamp: (content: string, version: string) => string | null
    /** Extract the currently stamped version for --verify. */
    current: (content: string) => string | null
    /** Version string this file should carry. */
    expected: (version: string) => string
    /** Skip this target entirely (with a note) for the given version. */
    skip?: (version: string) => string | null
}

function lineReplace(pattern: RegExp, render: (version: string) => string) {
    return (content: string, version: string): string | null => {
        if (!pattern.test(content)) return null
        return content.replace(pattern, render(version))
    }
}

const TARGETS: Target[] = [
    {
        path: 'Cargo.toml',
        // Only the [workspace.package] section carries a literal version.
        stamp: lineReplace(/^version = "[^"]+"$/m, (v) => `version = "${v}"`),
        current: (c) => /^version = "([^"]+)"$/m.exec(c)?.[1] ?? null,
        expected: (v) => v,
    },
    {
        // Excluded from the root workspace (standalone trunk/WASM build), so
        // it cannot inherit [workspace.package] and needs its own stamp.
        path: 'crates/hypercolor-ui/Cargo.toml',
        stamp: lineReplace(/^version = "[^"]+"$/m, (v) => `version = "${v}"`),
        current: (c) => /^version = "([^"]+)"$/m.exec(c)?.[1] ?? null,
        expected: (v) => v,
    },
    {
        path: 'crates/hypercolor-app/tauri.conf.json',
        stamp: lineReplace(/"version": "[^"]+"/, (v) => `"version": "${v}"`),
        current: (c) => /"version": "([^"]+)"/.exec(c)?.[1] ?? null,
        expected: (v) => v,
    },
    {
        path: 'python/pyproject.toml',
        stamp: lineReplace(/^version = "[^"]+"$/m, (v) => `version = "${pep440(v)}"`),
        current: (c) => /^version = "([^"]+)"$/m.exec(c)?.[1] ?? null,
        expected: (v) => pep440(v),
    },
    {
        path: 'packaging/aur/PKGBUILD',
        stamp: lineReplace(/^pkgver=.*$/m, (v) => `pkgver=${v}`),
        current: (c) => /^pkgver=(.*)$/m.exec(c)?.[1] ?? null,
        expected: (v) => v,
        skip: (v) => (v.includes('-') ? 'prerelease — AUR pkgver forbids hyphens; PKGBUILD tracks stable releases only' : null),
    },
    {
        path: 'sdk/packages/core/package.json',
        stamp: lineReplace(/"version": "[^"]+"/, (v) => `"version": "${v}"`),
        current: (c) => /"version": "([^"]+)"/.exec(c)?.[1] ?? null,
        expected: (v) => v,
    },
    {
        path: 'sdk/packages/create-effect/package.json',
        stamp: lineReplace(/"version": "[^"]+"/, (v) => `"version": "${v}"`),
        current: (c) => /"version": "([^"]+)"/.exec(c)?.[1] ?? null,
        expected: (v) => v,
    },
]

function main(): void {
    const [versionArg, modeArg] = process.argv.slice(2)
    if (!versionArg) fail('usage: bun scripts/set-version.ts <version> [--verify]')
    const version = versionArg.replace(/^v/, '')
    if (!SEMVER.test(version)) fail(`not a valid semver version: ${version}`)
    const verify = modeArg === '--verify'

    let failures = 0
    for (const target of TARGETS) {
        const path = resolve(REPO_ROOT, target.path)
        const skipReason = target.skip?.(version)
        if (skipReason) {
            console.log(`- ${target.path}: skipped (${skipReason})`)
            continue
        }

        const content = readFileSync(path, 'utf8')
        const expected = target.expected(version)

        if (verify) {
            const current = target.current(content)
            if (current === expected) {
                console.log(`✓ ${target.path}: ${current}`)
            } else {
                console.error(`✗ ${target.path}: expected ${expected}, found ${current ?? 'nothing'}`)
                failures += 1
            }
            continue
        }

        const stamped = target.stamp(content, version)
        if (stamped === null) {
            console.error(`✗ ${target.path}: version pattern not found`)
            failures += 1
            continue
        }
        writeFileSync(path, stamped)
        console.log(`✓ ${target.path} → ${expected}`)
    }

    if (failures > 0) fail(`${failures} file(s) failed`)
    console.log(verify ? `\nAll version files agree on ${version}.` : `\nStamped ${version}. Refresh lockfiles next.`)
}

main()
