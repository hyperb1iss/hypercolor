import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { readCurrentMacos, renderCask, renderFormula } from '../homebrew-formula.mjs';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const script = path.join(repo, 'scripts/homebrew-formula.mjs');
const templatePath = path.join(repo, 'packaging/homebrew/hypercolor.rb');
const caskPath = path.join(repo, 'packaging/homebrew/hypercolor-app.rb');
const template = readFileSync(templatePath, 'utf8');
const caskTemplate = readFileSync(caskPath, 'utf8');
const sha = seed => seed.repeat(64);
const linux = { amd64: sha('a'), arm64: sha('b') };
// A release that shipped the complete signed macOS set alongside Linux.
const macos = { version: '0.5.2', shas: { amd64: sha('c'), arm64: sha('d') } };

// The formula the tap carried before the public lane owned Linux: one
// top-level version and a single macOS tarball.
const legacyTap = `class Hypercolor < Formula
  desc "Open-source RGB lighting orchestration engine"
  homepage "https://github.com/hyperb1iss/hypercolor"
  version "0.3.2"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-arm64.tar.gz"
      sha256 "${sha('c')}"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-linux-amd64.tar.gz"
      sha256 "${sha('d')}"
    end
  end
end
`;

test('all four formula downloads use the same version and their own checksum', () => {
  const formula = renderFormula({ template, version: '0.5.2', linux, macos });
  assert.deepEqual([...formula.matchAll(/version "([^"]+)"/g)].map(match => match[1]), ['0.5.2', '0.5.2']);
  for (const [platform, checksums] of Object.entries({ linux, macos: macos.shas })) {
    for (const [arch, checksum] of Object.entries(checksums)) {
      assert.ok(formula.includes(`-${platform}-${arch}.tar.gz"\n      sha256 "${checksum}"`));
    }
  }
  assert.doesNotMatch(formula, /PLACEHOLDER|SHA256_/);
});

test('formula uses valid Homebrew requirements and service paths', () => {
  const formula = renderFormula({ template, version: '0.5.2', linux, macos });
  assert.match(formula, /depends_on macos: :sequoia/);
  assert.doesNotMatch(formula, /depends_on macos: "/);
  const macService = formula.match(/on_macos do\n    service do\n([\s\S]*?)^    end\n/m)[1];
  const linuxService = formula.match(/on_linux do\n    service do\n([\s\S]*?)^    end\n/m)[1];
  for (const service of [macService, linuxService]) {
    assert.match(service, /"--ui-dir", opt_pkgshare\/"ui"/);
    assert.doesNotMatch(service, /\bshare\//);
  }
  assert.match(macService, /"--macos-owner", "homebrew"/);
  assert.doesNotMatch(linuxService, /macos-owner|HYPERCOLOR_SERVICE_IDENTITY|HYPERCOLOR_MACOS_OWNER/);
});

test('formula rejects incomplete releases and unsupported version forms', () => {
  assert.throws(() => renderFormula({ template, version: '0.5.2-rc.1', linux, macos }), /stable X\.Y\.Z/);
  for (const arch of ['amd64', 'arm64']) {
    assert.throws(() => renderFormula({ template, version: '0.5.2', linux: { ...linux, [arch]: undefined }, macos }),
      new RegExp(`linux ${arch} sha256`));
    assert.throws(() => renderFormula({ template, version: '0.5.2', linux,
      macos: { version: '0.5.2', shas: { ...macos.shas, [arch]: 'nope' } } }), new RegExp(`macos ${arch} sha256`));
  }
  assert.throws(() => renderFormula({ template, version: '0.5.2', linux, macos: { version: '0.5.2', shas: {} } }),
    /at least one architecture/);
  assert.throws(() => renderFormula({ template, version: '0.5.2', linux, macos: { version: '0.5.2-rc.1', shas: macos.shas } }),
    /macOS version/);
  assert.throws(() => renderFormula({ template: template.replace('SHA256_LINUX_ARM64', ''), version: '0.5.2', linux, macos }),
    /missing the SHA256_LINUX_ARM64/);
  assert.throws(() => renderFormula({ template: template.replace('MACOS_VERSION_PLACEHOLDER', ''), version: '0.5.2', linux, macos }),
    /missing the MACOS_VERSION_PLACEHOLDER/);
});

test('readCurrentMacos reads a legacy single-version formula', () => {
  assert.deepEqual(readCurrentMacos(legacyTap), { version: '0.3.2', shas: { arm64: sha('c') } });
});

test('readCurrentMacos prefers the version declared inside on_macos', () => {
  const rendered = renderFormula({ template, version: '0.5.2', linux,
    macos: { version: '0.3.2', shas: { arm64: sha('c'), amd64: sha('e') } } });
  assert.deepEqual(readCurrentMacos(rendered), { version: '0.3.2', shas: { arm64: sha('c'), amd64: sha('e') } });
});

test('readCurrentMacos returns undefined without a macOS tarball', () => {
  const linuxOnly = renderFormula({ template, version: '0.5.2', linux, macos: undefined });
  assert.equal(readCurrentMacos(linuxOnly), undefined);
  assert.equal(readCurrentMacos('class Hypercolor < Formula\n  version "0.1.0"\nend\n'), undefined);
  assert.throws(() => readCurrentMacos(legacyTap.replace('version "0.3.2"', 'version "0.3.2-rc.1"')), /current macOS version/);
});

test('Linux stanzas take the release and macOS stanzas are carried forward', () => {
  const formula = renderFormula({ template, version: '0.5.2', linux, macos: readCurrentMacos(legacyTap) });
  assert.match(formula, /^  version "0\.5\.2"$/m);
  assert.match(formula, /^    version "0\.3\.2"$/m);
  assert.ok(formula.includes(`hypercolor-#{version}-linux-amd64.tar.gz"\n      sha256 "${sha('a')}"`));
  assert.ok(formula.includes(`hypercolor-#{version}-linux-arm64.tar.gz"\n      sha256 "${sha('b')}"`));
  assert.ok(formula.includes(`hypercolor-#{version}-macos-arm64.tar.gz"\n      sha256 "${sha('c')}"`));
  assert.ok(!formula.includes('macos-amd64'), 'an architecture the tap never published is left out');
  assert.match(formula, /depends_on macos: :sequoia/);
  assert.doesNotMatch(formula, /PLACEHOLDER|SHA256_/);
});

test('both macOS architectures render as an if/elsif chain', () => {
  const formula = renderFormula({ template, version: '0.5.2', linux,
    macos: { version: '0.4.0', shas: { arm64: sha('c'), amd64: sha('e') } } });
  const block = formula.match(/^  on_macos do\n([\s\S]*?)^  end\n/m)[1];
  assert.match(block, /    if Hardware::CPU\.arm\?\n      url [^\n]*macos-arm64[^\n]*\n      sha256 "c{64}"\n    elsif Hardware::CPU\.intel\?\n      url [^\n]*macos-amd64[^\n]*\n      sha256 "e{64}"\n    end\n/);
});

test('a tap without macOS artifacts renders a Linux-only formula', () => {
  const formula = renderFormula({ template, version: '0.5.2', linux, macos: undefined });
  assert.ok(!formula.includes('macos-arm64.tar.gz'));
  assert.ok(!formula.includes('MacosVersionRequirement\n\n    if'), 'the artifact block is gone');
  assert.match(formula, /on_macos do\n    service do/, 'the macOS service stanza stays for the signed lane');
  assert.match(formula, /on_linux do\n    service do/);
  assert.doesNotMatch(formula, /PLACEHOLDER|SHA256_/);
});

test('cask requires both published DMG checksums', () => {
  const cask = renderCask({ template: caskTemplate, version: '0.5.2', arm64: sha('e'), x86_64: sha('f') });
  assert.match(cask, /version "0\.5\.2"/);
  assert.match(cask, /sha256 arm: +"e{64}",\n +intel: "f{64}"/);
  assert.throws(() => renderCask({ template: caskTemplate, version: '0.5.2', arm64: sha('e') }), /DMG x86_64 sha256/);
  assert.throws(() => renderCask({ template: caskTemplate, version: '0.5.2', arm64: 'invalid', x86_64: sha('f') }), /DMG arm64 sha256/);
});

test('CLI validates both packages before writing either output', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-formula-'));
  try {
    const formulaOutput = path.join(dir, 'hypercolor.rb');
    const caskOutput = path.join(dir, 'hypercolor-app.rb');
    const args = [script, '--version', '0.5.2', '--template', templatePath, '--cask-template', caskPath,
      '--linux-amd64', linux.amd64, '--linux-arm64', linux.arm64,
      '--macos-amd64', macos.shas.amd64, '--macos-arm64', macos.shas.arm64,
      '--dmg-arm64', sha('e'), '--dmg-x86_64', 'invalid',
      '--output', formulaOutput, '--cask-output', caskOutput];
    const invalid = spawnSync(process.execPath, args, { encoding: 'utf8' });
    assert.equal(invalid.status, 1);
    assert.match(invalid.stderr, /DMG x86_64 sha256/);
    assert.equal(existsSync(formulaOutput), false);
    assert.equal(existsSync(caskOutput), false);
    args[args.indexOf('invalid')] = sha('f');
    const valid = spawnSync(process.execPath, args, { encoding: 'utf8' });
    assert.equal(valid.status, 0, valid.stderr);
    assert.match(valid.stdout, /wrote formula and cask for 0\.5\.2/);
    assert.equal(readFileSync(formulaOutput, 'utf8'), renderFormula({ template, version: '0.5.2', linux, macos }));
    assert.equal(readFileSync(caskOutput, 'utf8'), renderCask({ template: caskTemplate, version: '0.5.2', arm64: sha('e'), x86_64: sha('f') }));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('CLI carries the tap macOS stanzas forward when the release shipped none', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-formula-'));
  try {
    const current = path.join(dir, 'current.rb');
    const output = path.join(dir, 'hypercolor.rb');
    const caskOutput = path.join(dir, 'hypercolor-app.rb');
    writeFileSync(current, legacyTap);
    const base = [script, '--version', '0.5.2', '--template', templatePath,
      '--linux-amd64', linux.amd64, '--linux-arm64', linux.arm64, '--output', output];
    const carried = spawnSync(process.execPath, [...base, '--current', current], { encoding: 'utf8' });
    assert.equal(carried.status, 0, carried.stderr);
    assert.match(carried.stdout, /macOS stanzas carried at 0\.3\.2 for arm64/);
    assert.equal(readFileSync(output, 'utf8'), renderFormula({ template, version: '0.5.2', linux, macos: readCurrentMacos(legacyTap) }));
    assert.equal(existsSync(caskOutput), false, 'the cask is left alone when no DMG was published');

    rmSync(current);
    const fresh = spawnSync(process.execPath, [...base, '--current', current], { encoding: 'utf8' });
    assert.equal(fresh.status, 0, fresh.stderr);
    assert.match(fresh.stdout, /Linux only, the tap has no macOS build to carry/);
    assert.equal(readFileSync(output, 'utf8'), renderFormula({ template, version: '0.5.2', linux, macos: undefined }));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('CLI rejects a partial macOS release instead of advancing the tap', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-formula-'));
  try {
    const output = path.join(dir, 'hypercolor.rb');
    const partial = spawnSync(process.execPath, [script, '--version', '0.5.2', '--template', templatePath,
      '--linux-amd64', linux.amd64, '--linux-arm64', linux.arm64, '--output', output,
      '--macos-amd64', macos.shas.amd64, '--macos-arm64', macos.shas.arm64], { encoding: 'utf8' });
    assert.equal(partial.status, 1);
    assert.match(partial.stderr, /partial macOS release: --dmg-arm64, --dmg-x86_64, --cask-template, --cask-output missing/);
    assert.equal(existsSync(output), false);

    const missing = spawnSync(process.execPath, [script, '--version', '0.5.2'], { encoding: 'utf8' });
    assert.equal(missing.status, 1);
    assert.match(missing.stderr, /--linux-amd64 is required/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
