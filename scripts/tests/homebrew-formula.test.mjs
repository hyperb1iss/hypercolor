import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { readCurrentMacos, renderFormula } from '../homebrew-formula.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '..', '..');
const script = path.join(repo, 'scripts', 'homebrew-formula.mjs');
const template = readFileSync(path.join(repo, 'packaging', 'homebrew', 'hypercolor.rb'), 'utf8');

const sha = (seed) => seed.repeat(64).slice(0, 64);
const linux = { amd64: sha('a'), arm64: sha('b') };

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

test('readCurrentMacos reads a legacy single-version formula', () => {
  assert.deepEqual(readCurrentMacos(legacyTap), { version: '0.3.2', shas: { arm64: sha('c') } });
});

test('readCurrentMacos prefers the version declared inside on_macos', () => {
  const rendered = renderFormula({ template, version: '0.5.0', linux, macos: { version: '0.3.2', shas: { arm64: sha('c'), amd64: sha('e') } } });
  assert.deepEqual(readCurrentMacos(rendered), { version: '0.3.2', shas: { arm64: sha('c'), amd64: sha('e') } });
});

test('readCurrentMacos returns undefined without a macOS tarball', () => {
  const linuxOnly = renderFormula({ template, version: '0.5.0', linux, macos: undefined });
  assert.equal(readCurrentMacos(linuxOnly), undefined);
  assert.equal(readCurrentMacos('class Hypercolor < Formula\n  version "0.1.0"\nend\n'), undefined);
});

test('Linux stanzas take the release and macOS stanzas are carried forward', () => {
  const formula = renderFormula({ template, version: '0.5.0', linux, macos: readCurrentMacos(legacyTap) });
  assert.match(formula, /^  version "0\.5\.0"$/m);
  assert.match(formula, /^    version "0\.3\.2"$/m);
  assert.ok(formula.includes(`hypercolor-#{version}-linux-amd64.tar.gz"\n      sha256 "${sha('a')}"`));
  assert.ok(formula.includes(`hypercolor-#{version}-linux-arm64.tar.gz"\n      sha256 "${sha('b')}"`));
  assert.ok(formula.includes(`hypercolor-#{version}-macos-arm64.tar.gz"\n      sha256 "${sha('c')}"`));
  assert.ok(!formula.includes('macos-amd64'), 'an architecture the tap never published is left out');
  assert.ok(!formula.includes('elsif Hardware::CPU.intel?\n      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos'));
  assert.match(formula, /depends_on macos: ">= :sequoia"/);
  assert.doesNotMatch(formula, /PLACEHOLDER|SHA256_/);
});

test('both macOS architectures render as an if/elsif chain', () => {
  const formula = renderFormula({ template, version: '0.5.0', linux, macos: { version: '0.4.0', shas: { arm64: sha('c'), amd64: sha('e') } } });
  const block = formula.match(/^  on_macos do\n([\s\S]*?)^  end\n/m)[1];
  assert.match(block, /    if Hardware::CPU\.arm\?\n      url [^\n]*macos-arm64[^\n]*\n      sha256 "c{64}"\n    elsif Hardware::CPU\.intel\?\n      url [^\n]*macos-amd64[^\n]*\n      sha256 "e{64}"\n    end\n/);
});

test('a tap without macOS artifacts renders a Linux-only formula', () => {
  const formula = renderFormula({ template, version: '0.5.0', linux, macos: undefined });
  assert.ok(!formula.includes('macos-arm64.tar.gz'));
  assert.ok(!formula.includes('MacosVersionRequirement\n\n    if'), 'the artifact block is gone');
  assert.match(formula, /on_macos do\n    service do/, 'the macOS service stanza stays for the signed lane');
  assert.match(formula, /on_linux do\n    service do/);
  assert.doesNotMatch(formula, /PLACEHOLDER|SHA256_/);
});

test('the Linux service never passes the macOS-only owner flag', () => {
  const formula = renderFormula({ template, version: '0.5.0', linux, macos: undefined });
  const linuxService = formula.match(/on_linux do\n    service do\n([\s\S]*?)^    end\n/m)[1];
  assert.doesNotMatch(linuxService, /macos-owner|HYPERCOLOR_SERVICE_IDENTITY|HYPERCOLOR_MACOS_OWNER/);
  const macosService = formula.match(/on_macos do\n    service do\n([\s\S]*?)^    end\n/m)[1];
  assert.match(macosService, /"--macos-owner", "homebrew"/);
});

test('invalid inputs are rejected', () => {
  assert.throws(() => renderFormula({ template, version: '0.5.0-rc.1', linux, macos: undefined }), /stable X\.Y\.Z/);
  assert.throws(() => renderFormula({ template, version: '0.5.0', linux: { amd64: 'nope', arm64: sha('b') }, macos: undefined }), /sha256/);
  assert.throws(() => renderFormula({ template: template.replace('SHA256_LINUX_ARM64', ''), version: '0.5.0', linux, macos: undefined }), /SHA256_LINUX_ARM64/);
  assert.throws(() => readCurrentMacos(legacyTap.replace('version "0.3.2"', 'version "0.3.2-rc.1"')), /current macOS version/);
});

test('the CLI renders from the checked-in template against a live tap formula', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-formula-'));
  try {
    const current = path.join(dir, 'current.rb');
    const output = path.join(dir, 'hypercolor.rb');
    writeFileSync(current, legacyTap);
    const result = spawnSync(process.execPath, [
      script,
      '--version', '0.5.0',
      '--linux-amd64', linux.amd64,
      '--linux-arm64', linux.arm64,
      '--template', path.join(repo, 'packaging', 'homebrew', 'hypercolor.rb'),
      '--current', current,
      '--output', output,
    ], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /macOS stanzas carried at 0\.3\.2 for arm64/);
    assert.equal(readFileSync(output, 'utf8'), renderFormula({ template, version: '0.5.0', linux, macos: readCurrentMacos(legacyTap) }));

    const missing = spawnSync(process.execPath, [script, '--version', '0.5.0'], { encoding: 'utf8' });
    assert.equal(missing.status, 1);
    assert.match(missing.stderr, /--linux-amd64 is required/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
