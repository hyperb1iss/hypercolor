import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { renderFormula, renderCask } from '../homebrew-formula.mjs';

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
