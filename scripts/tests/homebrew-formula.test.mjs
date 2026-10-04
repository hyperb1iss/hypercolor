import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { readPublishedMacos, renderFormula, renderCask } from '../homebrew-formula.mjs';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const script = path.join(repo, 'scripts/homebrew-formula.mjs');
const templatePath = path.join(repo, 'packaging/homebrew/hypercolor.rb');
const caskPath = path.join(repo, 'packaging/homebrew/hypercolor-app.rb');
const template = readFileSync(templatePath, 'utf8');
const caskTemplate = readFileSync(caskPath, 'utf8');
const sha = seed => seed.repeat(64);
const linux = { amd64: sha('a'), arm64: sha('b') };
const macos = { arm64: sha('d') };

test('all three formula downloads use the same version and their own checksum', () => {
  const formula = renderFormula({ template, version: '0.5.2', linux, macos });
  assert.deepEqual([...formula.matchAll(/version "([^"]+)"/g)].map(match => match[1]), ['0.5.2']);
  assert.equal([...formula.matchAll(/^\s+url "/gm)].length, 3);
  for (const [platform, checksums] of Object.entries({ linux, macos })) {
    for (const [arch, checksum] of Object.entries(checksums)) {
      assert.match(formula, new RegExp(`-${platform}-${arch}\\.tar\\.gz"\\n +sha256 "${checksum}"`));
    }
  }
  assert.doesNotMatch(formula, /PLACEHOLDER|SHA256_/);
});

test('formula refuses Intel Macs instead of fetching a missing archive', () => {
  const formula = renderFormula({ template, version: '0.5.2', linux, macos });
  const macDownload = formula.match(/^  on_macos do\n([\s\S]*?)^  end\n/m)[1];
  assert.match(macDownload, /^    depends_on arch: :arm64$/m);
  assert.match(macDownload, /-macos-arm64\.tar\.gz"/);
  assert.doesNotMatch(macDownload, /Hardware::CPU|intel|amd64|x86_64/i);
  assert.doesNotMatch(formula, /macos-amd64|SHA256_MACOS_AMD64/);
  // Linux keeps both architectures, so the arch requirement stays macOS-only.
  const linuxDownload = formula.match(/^  on_linux do\n([\s\S]*?)^  end\n/m)[1];
  assert.match(linuxDownload, /-linux-amd64\.tar\.gz"/);
  assert.match(linuxDownload, /-linux-arm64\.tar\.gz"/);
  assert.equal([...formula.matchAll(/depends_on arch:/g)].length, 1);
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
  for (const [platform, arch] of [['linux', 'amd64'], ['linux', 'arm64'], ['macos', 'arm64']]) {
    const inputs = { template, version: '0.5.2', linux, macos };
    inputs[platform] = { ...inputs[platform], [arch]: undefined };
    assert.throws(() => renderFormula(inputs), new RegExp(`${platform} ${arch} sha256`));
  }
  assert.throws(() => renderFormula({ template: template.replace('SHA256_MACOS_ARM64', ''),
    version: '0.5.2', linux, macos }), /missing the SHA256_MACOS_ARM64/);
  // A template that still names an Intel macOS archive never renders.
  assert.throws(() => renderFormula({ template: `${template}\n# SHA256_MACOS_AMD64\n`,
    version: '0.5.2', linux, macos }), /SHA256_MACOS_AMD64 survived rendering/);
});

test('cask ships the Apple silicon DMG and refuses Intel Macs', () => {
  const cask = renderCask({ template: caskTemplate, version: '0.5.2', arm64: sha('e') });
  assert.match(cask, /version "0\.5\.2"/);
  assert.match(cask, /^  sha256 "e{64}"$/m);
  assert.match(cask, /Hypercolor-#\{version\}-arm64\.dmg"/);
  assert.match(cask, /^  depends_on arch: :arm64$/m);
  assert.doesNotMatch(cask, /\bintel:|x86_64|#\{arch\}|^  arch /m);
  assert.throws(() => renderCask({ template: caskTemplate, version: '0.5.2' }), /DMG arm64 sha256/);
  assert.throws(() => renderCask({ template: caskTemplate, version: '0.5.2', arm64: 'invalid' }), /DMG arm64 sha256/);
  assert.throws(() => renderCask({ template: `${caskTemplate}\n# SHA256_MACOS_APP_X86_64\n`,
    version: '0.5.2', arm64: sha('e') }), /SHA256_MACOS_APP_X86_64 survived rendering/);
});

test('CLI validates both packages before writing either output', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-formula-'));
  try {
    const formulaOutput = path.join(dir, 'hypercolor.rb');
    const caskOutput = path.join(dir, 'hypercolor-app.rb');
    const args = [script, '--version', '0.5.2', '--template', templatePath, '--cask-template', caskPath,
      '--linux-amd64', linux.amd64, '--linux-arm64', linux.arm64,
      '--macos-arm64', macos.arm64, '--dmg-arm64', 'invalid',
      '--output', formulaOutput, '--cask-output', caskOutput];
    const run = argv => spawnSync(process.execPath, argv, { encoding: 'utf8' });
    const invalid = run(args);
    assert.equal(invalid.status, 1);
    assert.match(invalid.stderr, /DMG arm64 sha256/);
    assert.equal(existsSync(formulaOutput), false);
    assert.equal(existsSync(caskOutput), false);
    args[args.indexOf('invalid')] = sha('e');
    // Intel macOS inputs are refused outright rather than silently ignored.
    for (const intel of [['--macos-amd64', sha('c')], ['--dmg-x86_64', sha('f')]]) {
      const stale = run([...args, ...intel]);
      assert.equal(stale.status, 1);
      assert.match(stale.stderr, new RegExp(`unknown option ${intel[0]}`));
      assert.equal(existsSync(formulaOutput), false);
    }
    const valid = run(args);
    assert.equal(valid.status, 0, valid.stderr);
    assert.equal(readFileSync(formulaOutput, 'utf8'), renderFormula({ template, version: '0.5.2', linux, macos }));
    assert.equal(readFileSync(caskOutput, 'utf8'), renderCask({ template: caskTemplate, version: '0.5.2', arm64: sha('e') }));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// The tap formula as the 0.5.x carry-forward renderer published it: Linux on
// the release version, macOS pinned to an older signed build.
const legacyPublished = `class Hypercolor < Formula
  version "0.5.1"

  on_macos do
    version "0.3.2"
    depends_on macos: :sequoia

    if Hardware::CPU.arm?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-arm64.tar.gz"
      sha256 "${sha('9')}"
    end
  end
end
`;

test('carry-forward advances Linux and pins macOS to the published build', () => {
  const carried = readPublishedMacos(legacyPublished);
  assert.deepEqual(carried, { version: '0.3.2', arm64: sha('9') });
  const formula = renderFormula({ template, version: '0.6.1', linux, macos: { carried } });
  assert.match(formula, /^  version "0\.6\.1"$/m);
  const macDownload = formula.match(/^  on_macos do\n([\s\S]*?)^  end\n/m)[1];
  assert.match(macDownload, /^    version "0\.3\.2"\n    depends_on arch: :arm64$/m);
  assert.match(macDownload, new RegExp(`-macos-arm64\\.tar\\.gz"\\n +sha256 "${sha('9')}"`));
  const linuxDownload = formula.match(/^  on_linux do\n([\s\S]*?)^  end\n/m)[1];
  assert.doesNotMatch(linuxDownload, /version "/);
  assert.match(linuxDownload, new RegExp(`sha256 "${linux.amd64}"`));
  // The service block's on_macos stays untouched.
  assert.equal([...formula.matchAll(/^    version "/gm)].length, 1);
  assert.doesNotMatch(formula, /PLACEHOLDER|SHA256_/);
});

test('carry-forward is stable across consecutive releases', () => {
  const full = renderFormula({ template, version: '0.6.0', linux, macos });
  assert.deepEqual(readPublishedMacos(full), { version: '0.6.0', arm64: macos.arm64 });
  const once = renderFormula({ template, version: '0.6.1', linux, macos: { carried: readPublishedMacos(full) } });
  const twice = renderFormula({ template, version: '0.6.2', linux, macos: { carried: readPublishedMacos(once) } });
  assert.deepEqual(readPublishedMacos(twice), { version: '0.6.0', arm64: macos.arm64 });
  assert.equal([...twice.matchAll(/^    version "/gm)].length, 1);
});

test('carry-forward refuses a published formula it cannot read', () => {
  assert.throws(() => readPublishedMacos('class Hypercolor < Formula\nend\n'), /no on_macos download block/);
  const twoShas = legacyPublished.replace(/(sha256 "9+")/, `$1\n      sha256 "${sha('8')}"`);
  assert.throws(() => readPublishedMacos(twoShas), /exactly one sha256, found 2/);
  assert.throws(() => readPublishedMacos(legacyPublished.replace(sha('9'), 'invalid')), /published macOS arm64 sha256/);
  assert.throws(() => renderFormula({ template, version: '0.6.1', linux,
    macos: { carried: { version: '0.3.2-rc.1', arm64: sha('9') } } }), /carried macOS version/);
});

test('CLI carry mode writes only the formula and refuses mixed or partial macOS inputs', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-formula-carry-'));
  try {
    const published = path.join(dir, 'published.rb');
    writeFileSync(published, legacyPublished);
    const formulaOutput = path.join(dir, 'hypercolor.rb');
    const caskOutput = path.join(dir, 'hypercolor-app.rb');
    const base = [script, '--version', '0.6.1', '--template', templatePath,
      '--linux-amd64', linux.amd64, '--linux-arm64', linux.arm64, '--output', formulaOutput];
    const run = argv => spawnSync(process.execPath, argv, { encoding: 'utf8' });

    const mixed = run([...base, '--carry-macos-from', published, '--macos-arm64', macos.arm64]);
    assert.equal(mixed.status, 1);
    assert.match(mixed.stderr, /cannot be combined with --macos-arm64/);
    const partial = run([...base, '--macos-arm64', macos.arm64, '--cask-template', caskPath,
      '--cask-output', caskOutput]);
    assert.equal(partial.status, 1);
    assert.match(partial.stderr, /--dmg-arm64 is required unless --carry-macos-from/);
    const neither = run(base);
    assert.equal(neither.status, 1);
    assert.match(neither.stderr, /--macos-arm64 is required unless --carry-macos-from/);
    assert.equal(existsSync(formulaOutput), false);

    const carried = run([...base, '--carry-macos-from', published]);
    assert.equal(carried.status, 0, carried.stderr);
    assert.match(carried.stdout, /macOS stays on 0\.3\.2 and the cask is unchanged/);
    assert.equal(readFileSync(formulaOutput, 'utf8'), renderFormula({ template, version: '0.6.1', linux,
      macos: { carried: readPublishedMacos(legacyPublished) } }));
    assert.equal(existsSync(caskOutput), false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a withdrawn macOS stanza refuses macOS and keeps Linux on the release', () => {
  const formula = renderFormula({ template, version: '0.6.1', linux, macos: { withdrawn: true } });
  const macDownload = formula.match(/^  on_macos do\n([\s\S]*?)^  end\n/m)[1];
  assert.match(macDownload, /^    depends_on NotarizedMacosBuildRequirement$/m);
  // Homebrew needs a URL to load the formula on macOS; it names a real asset.
  assert.match(macDownload, new RegExp(`-linux-amd64\\.tar\\.gz"\\n +sha256 "${linux.amd64}"`));
  assert.doesNotMatch(macDownload, /macos-arm64|MacosVersionRequirement|depends_on arch|version "/);
  assert.doesNotMatch(formula, /Intel Macs up front/);
  assert.match(formula, /^  class NotarizedMacosBuildRequirement < Requirement$/m);
  assert.match(formula, /^  version "0\.6\.1"$/m);
  // The service block's on_macos stays as rendered for a full release.
  const full = renderFormula({ template, version: '0.6.1', linux, macos });
  const service = text => text.match(/on_macos do\n    service do\n[\s\S]*?^    end\n/m)[0];
  assert.equal(service(formula), service(full));
  assert.doesNotMatch(formula, /PLACEHOLDER|SHA256_/);
});

test('carry mode keeps a withdrawn stanza withdrawn until a notarized release restores macOS', () => {
  const withdrawn = renderFormula({ template, version: '0.6.1', linux, macos: { withdrawn: true } });
  assert.deepEqual(readPublishedMacos(withdrawn), { withdrawn: true });
  const carried = renderFormula({ template, version: '0.6.2', linux,
    macos: { carried: readPublishedMacos(withdrawn) } });
  assert.deepEqual(readPublishedMacos(carried), { withdrawn: true });
  assert.match(carried, /^  version "0\.6\.2"$/m);
  assert.equal([...carried.matchAll(/NotarizedMacosBuildRequirement$/gm)].length, 1);
  const restored = renderFormula({ template, version: '0.6.3', linux, macos });
  assert.deepEqual(readPublishedMacos(restored), { version: '0.6.3', arm64: macos.arm64 });
  assert.doesNotMatch(restored.match(/^  on_macos do\n([\s\S]*?)^  end\n/m)[1], /NotarizedMacosBuildRequirement/);
});

test('CLI withdraw mode writes only the formula and refuses other macOS inputs', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-formula-withdraw-'));
  try {
    const formulaOutput = path.join(dir, 'hypercolor.rb');
    const caskOutput = path.join(dir, 'hypercolor-app.rb');
    const published = path.join(dir, 'published.rb');
    writeFileSync(published, legacyPublished);
    const base = [script, '--version', '0.6.1', '--template', templatePath,
      '--linux-amd64', linux.amd64, '--linux-arm64', linux.arm64, '--output', formulaOutput];
    const run = argv => spawnSync(process.execPath, argv, { encoding: 'utf8' });
    for (const [extra, conflict] of [[['--carry-macos-from', published], 'carry-macos-from'],
      [['--macos-arm64', macos.arm64], 'macos-arm64']]) {
      const refused = run([...base, '--withdraw-macos', ...extra]);
      assert.equal(refused.status, 1);
      assert.match(refused.stderr, new RegExp(`--withdraw-macos cannot be combined with --${conflict}`));
      assert.equal(existsSync(formulaOutput), false);
    }
    const withdrawn = run([...base, '--withdraw-macos']);
    assert.equal(withdrawn.status, 0, withdrawn.stderr);
    assert.match(withdrawn.stdout, /macOS is withdrawn and the cask is unchanged/);
    assert.equal(readFileSync(formulaOutput, 'utf8'),
      renderFormula({ template, version: '0.6.1', linux, macos: { withdrawn: true } }));
    assert.equal(existsSync(caskOutput), false);
    // A later carry run reports that macOS stays withdrawn.
    const carried = run([script, '--version', '0.6.2', '--template', templatePath,
      '--linux-amd64', linux.amd64, '--linux-arm64', linux.arm64, '--output', path.join(dir, 'next.rb'),
      '--carry-macos-from', formulaOutput]);
    assert.equal(carried.status, 0, carried.stderr);
    assert.match(carried.stdout, /macOS stays withdrawn and the cask is unchanged/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
