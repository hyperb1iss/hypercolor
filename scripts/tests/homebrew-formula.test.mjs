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
const macos = { amd64: sha('c'), arm64: sha('d') };

test('all four formula downloads use the same version and their own checksum', () => {
  const formula = renderFormula({ template, version: '0.5.2', linux, macos });
  assert.deepEqual([...formula.matchAll(/version "([^"]+)"/g)].map(match => match[1]), ['0.5.2']);
  for (const [platform, checksums] of Object.entries({ linux, macos })) {
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
  for (const platform of ['linux', 'macos']) {
    for (const arch of ['amd64', 'arm64']) {
      const inputs = { template, version: '0.5.2', linux, macos };
      inputs[platform] = { ...inputs[platform], [arch]: undefined };
      assert.throws(() => renderFormula(inputs), new RegExp(`${platform} ${arch} sha256`));
    }
  }
  assert.throws(() => renderFormula({ template: template.replace('SHA256_MACOS_AMD64', ''),
    version: '0.5.2', linux, macos }), /missing the SHA256_MACOS_AMD64/);
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
      '--macos-amd64', macos.amd64, '--macos-arm64', macos.arm64,
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
    assert.equal(readFileSync(formulaOutput, 'utf8'), renderFormula({ template, version: '0.5.2', linux, macos }));
    assert.equal(readFileSync(caskOutput, 'utf8'), renderCask({ template: caskTemplate, version: '0.5.2', arm64: sha('e'), x86_64: sha('f') }));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
