import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const wrapper = fileURLToPath(new URL('../with-macos-signing.sh', import.meta.url));
const secretNames = ['APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_SIGNING_IDENTITY',
  'APPLE_TEAM_ID', 'APPLE_API_KEY_ID', 'APPLE_API_ISSUER', 'APPLE_API_KEY_CONTENT'];
const environment = { ...process.env };
for (const name of secretNames) delete environment[name];

test('signing fails before executing when credentials are incomplete', () => {
  const result = spawnSync('bash', [wrapper, 'echo', 'must-not-run'], { env: environment, encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /requires APPLE_CERTIFICATE/);
  assert.equal(result.stdout, '');
});

for (const exitCode of [0, 7]) {
  test(`notary key is private and removed after child exits ${exitCode}`, () => {
    const env = { ...environment, ...Object.fromEntries(secretNames.map(name => [name, 'fixture-only'])) };
    const result = spawnSync('bash', [wrapper, process.execPath, '-e', `
      const fs = require('node:fs');
      const path = process.env.APPLE_API_KEY_PATH;
      if ((fs.statSync(path).mode & 0o777) !== 0o600) process.exit(90);
      if (fs.readFileSync(path, 'utf8') !== 'fixture-only') process.exit(91);
      if (process.env.APPLE_API_KEY_CONTENT !== undefined) process.exit(92);
      console.log(path);
      process.exit(${exitCode});
    `], { env, encoding: 'utf8' });
    assert.equal(result.status, exitCode, result.stderr);
    const path = result.stdout.trim();
    assert.ok(path.endsWith('/AuthKey.p8'));
    assert.equal(existsSync(path), false);
  });
}

test('release pipeline publishes signed macOS packages and refreshes the cask', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  assert.match(workflow, /with-macos-signing\.sh scripts\/sign-macos-artifacts\.sh app/);
  assert.match(workflow, /with-macos-signing\.sh scripts\/dist\.sh/);
  assert.match(workflow, /sign-macos-artifacts\.sh verify-app/);
  assert.match(workflow, /sign-macos-artifacts\.sh verify-standalone/);
  assert.match(workflow, /-name '\*\.dmg'/);
  assert.match(workflow, /git add Formula\/hypercolor\.rb Casks\/hypercolor-app\.rb/);
  assert.doesNotMatch(workflow, /unsigned-app|oss-ci-\$/);
});

test('native app version validation accepts an exact stamped prerelease', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const job = workflow.match(/^  build-native-app:\n([\s\S]*?)(?=^  [a-z][\w-]*:)/m)[1];
  const step = job.match(/      - name: Determine version\n([\s\S]*?)(?=      - name:)/)[1];
  assert.match(step, /if \(\$version -ne \$cargoVersion -and \$baseVersion -ne \$cargoVersion\)/);
});

test('Homebrew checksum step supplies every value consumed by its renderer', () => {
  const repo = fileURLToPath(new URL('../../', import.meta.url));
  const workflow = readFileSync(path.join(repo, '.github/workflows/ci.yml'), 'utf8');
  const job = workflow.match(/^  update-homebrew:\n([\s\S]*?)(?=^  [a-z][\w-]*:|$(?![\s\S]))/m)?.[1];
  assert.ok(job, 'Homebrew publication job exists');
  const steps = new Map([...job.matchAll(/^      - name: (.+)\n([\s\S]*?)(?=^      - |$(?![\s\S]))/gm)]
    .map(([, name, body]) => [name, body]));
  const shell = body => {
    assert.ok(body, 'expected workflow step exists');
    const run = body.match(/^        run: \|\n([\s\S]*)/m)?.[1];
    assert.ok(run, 'step has a shell body');
    return run.replace(/^          /gm, '').replaceAll('${{ github.repository }}', 'hyperb1iss/hypercolor');
  };
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-workflow-'));
  try {
    const output = path.join(dir, 'outputs');
    const env = { ...process.env, VERSION: '0.5.2', GITHUB_OUTPUT: output };
    // Substitute only the download transport; execute the workflow shell itself.
    const gh = `gh() {
      local artifact='' directory=''
      while (( $# )); do
        case "$1" in
          --pattern) artifact="$2"; shift ;;
          --dir) directory="$2"; shift ;;
        esac
        shift
      done
      test -n "$artifact" && test -n "$directory" || return 99
      printf 'fixture:%s' "$artifact" > "$directory/$artifact"
    }
    `;
    const checksums = spawnSync('bash', ['-c', gh + shell(steps.get('Download release tarballs and compute checksums'))],
      { cwd: dir, env, encoding: 'utf8' });
    assert.equal(checksums.status, 0, checksums.stderr);
    const values = Object.fromEntries(readFileSync(output, 'utf8').trim().split('\n').map(line => line.split('=')));
    const expectedAssets = {
      sha256_linux_amd64: 'hypercolor-0.5.2-linux-amd64.tar.gz',
      sha256_linux_arm64: 'hypercolor-0.5.2-linux-arm64.tar.gz',
      sha256_macos_amd64: 'hypercolor-0.5.2-macos-amd64.tar.gz',
      sha256_macos_arm64: 'hypercolor-0.5.2-macos-arm64.tar.gz',
      sha256_dmg_arm64: 'Hypercolor-0.5.2-arm64.dmg',
      sha256_dmg_x86_64: 'Hypercolor-0.5.2-x86_64.dmg',
    };
    assert.deepEqual(Object.keys(values).sort(), Object.keys(expectedAssets).sort());
    for (const [key, asset] of Object.entries(expectedAssets)) {
      assert.equal(values[key], createHash('sha256').update(`fixture:${asset}`).digest('hex'));
    }
    const renderStep = steps.get('Render formula and cask');
    for (const [, name, key] of renderStep.matchAll(/^          (SHA256_\w+): \$\{\{ steps.checksums.outputs.(\w+) \}\}/gm)) {
      assert.ok(values[key], `renderer input ${name} has an upstream value`);
      env[name] = values[key];
    }
    mkdirSync(path.join(dir, 'scripts'));
    copyFileSync(path.join(repo, 'scripts/homebrew-formula.mjs'), path.join(dir, 'scripts/homebrew-formula.mjs'));
    symlinkSync(path.join(repo, 'packaging'), path.join(dir, 'packaging'));
    const rendered = spawnSync('bash', ['-c', shell(renderStep)], { cwd: dir, env, encoding: 'utf8' });
    assert.equal(rendered.status, 0, rendered.stderr);
    for (const file of ['Formula/hypercolor.rb', 'Casks/hypercolor-app.rb']) {
      const content = readFileSync(path.join(dir, 'homebrew-tap', file), 'utf8');
      assert.match(content, /version "0\.5\.2"/);
      assert.doesNotMatch(content, /PLACEHOLDER|SHA256_/);
    }
    const aur = workflow.match(/^  update-aur:\n([\s\S]*?)(?=^  [a-z][\w-]*:)/m)[1];
    assert.doesNotMatch(aur, /macos-|\.dmg/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('macOS tarballs package the native job binaries for both architectures', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const native = workflow.match(/^  build-native-app:\n([\s\S]*?)(?=^  [a-z][\w-]*:)/m)[1];
  const tarballs = workflow.match(/^  build-release:\n([\s\S]*?)(?=^  [a-z][\w-]*:)/m)[1];
  assert.doesNotMatch(tarballs, /target: macos-/);
  const step = native.match(/      - name: Assemble signed macOS distribution\n([\s\S]*?)(?=      - name:)/)[1];
  assert.match(step, /if: runner.os == 'macOS'/);
  const script = step.split('        run: |\n')[1].replace(/^          /gm, '');
  assert.doesNotMatch(script, /cargo build|cargo tauri/);
  assert.match(native, /name: hypercolor-tarball-\$\{\{ steps.version.outputs.version \}\}-\$\{\{ matrix.target \}\}/);
  assert.match(native, /dist\/hypercolor-\*\.tar.gz\n/);
  for (const [arch, target, platform] of [
    ['arm64', 'aarch64-apple-darwin', 'macos-arm64'],
    ['x86_64', 'x86_64-apple-darwin', 'macos-amd64'],
  ]) {
    const dir = mkdtempSync(path.join(tmpdir(), 'macos-prebuilt-release-'));
    try {
      mkdirSync(path.join(dir, 'scripts'));
      mkdirSync(path.join(dir, 'target/release'), { recursive: true });
      mkdirSync(path.join(dir, `target/${target}/release`), { recursive: true });
      for (const binary of ['hypercolor-daemon', 'hypercolor']) {
        writeFileSync(path.join(dir, 'target/release', binary), binary, { mode: 0o755 });
      }
      writeFileSync(path.join(dir, `target/${target}/release/hypercolor-app`), 'hypercolor-app', { mode: 0o755 });
      writeFileSync(path.join(dir, 'scripts/with-macos-signing.sh'), '#!/bin/bash\nexec "$@"\n');
      // Replace the platform signing transport; execute the actual workflow's
      // path selection, staging, package invocation and checksum generation.
      writeFileSync(path.join(dir, 'scripts/dist.sh'), `#!/bin/bash
set -euo pipefail
bin_dir=''
while (( $# )); do
  case "$1" in
    --bin-dir) bin_dir="$2"; shift ;;
    --target) test "$2" = '${target}'; shift ;;
    --version) test "$2" = '0.5.2'; shift ;;
    --web-assets) shift ;;
  esac
  shift
done
for binary in hypercolor-daemon hypercolor hypercolor-app; do
  test -x "$bin_dir/$binary"
  test "$(cat "$bin_dir/$binary")" = "$binary"
done
mkdir -p dist/hypercolor-0.5.2-${platform}
printf 'archive-fixture' > dist/hypercolor-0.5.2-${platform}.tar.gz
`, { mode: 0o755 });
      writeFileSync(path.join(dir, 'scripts/sign-macos-artifacts.sh'), `#!/bin/bash
set -euo pipefail
test "$1" = verify-standalone
test "$3" = dist/hypercolor-0.5.2-${platform}
test "$5" = '${target}'
test "$7" = fixture-team
`, { mode: 0o755 });
      const run = script.replaceAll('${{ matrix.rust-target }}', target)
        .replaceAll('${{ matrix.cask_arch }}', arch)
        .replaceAll('${{ steps.version.outputs.version }}', '0.5.2');
      const result = spawnSync('bash', ['-e', '-c', run], {
        cwd: dir, encoding: 'utf8',
        env: { ...environment, RUNNER_TEMP: dir, CARGO_TARGET_DIR: path.join(dir, 'target'), APPLE_TEAM_ID: 'fixture-team' },
      });
      assert.equal(result.status, 0, result.stderr);
      const checksum = readFileSync(path.join(dir, `dist/hypercolor-0.5.2-${platform}.tar.gz.sha256`), 'utf8');
      assert.equal(checksum.trim(), `${createHash('sha256').update('archive-fixture').digest('hex')}  hypercolor-0.5.2-${platform}.tar.gz`);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
});
