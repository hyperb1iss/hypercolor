import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { renderFormula } from '../homebrew-formula.mjs';

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
  assert.match(workflow, /with-macos-signing\.sh scripts\/sign-macos-artifacts\.sh sign-app/);
  assert.match(workflow, /with-macos-signing\.sh scripts\/dist\.sh/);
  assert.match(workflow, /sign-macos-artifacts\.sh verify-app/);
  assert.match(workflow, /sign-macos-artifacts\.sh verify-standalone/);
  assert.match(workflow, /-name '\*\.dmg'/);
  assert.match(workflow, /git add Formula\/hypercolor\.rb Casks\/hypercolor-app\.rb/);
  assert.doesNotMatch(workflow, /unsigned-app|oss-ci-\$/);
});

test('macOS signing runs in its own job from an unsigned build payload', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const body = id => workflow.match(new RegExp(`^  ${id}:\\n([\\s\\S]*?)(?=^  [a-z][\\w-]*:)`, 'm'))?.[1];
  const build = body('build-native-app');
  const sign = body('sign-macos');
  assert.ok(build && sign, 'build and signing jobs exist');
  // The four-hour build never holds Apple credentials or waits on Apple.
  assert.doesNotMatch(build, /APPLE_|with-macos-signing|sign-macos-artifacts\.sh/);
  assert.match(build, /cargo tauri build --ci --bundles app --no-sign/);
  assert.match(build, /name: unsigned-\$\{\{ matrix\.target \}\}/);
  assert.match(sign, /^    needs: \[build-native-app, release-credentials, web-assets\]$/m);
  assert.match(sign, /^    timeout-minutes: (\d+)$/m);
  assert.ok(Number(sign.match(/^    timeout-minutes: (\d+)$/m)[1]) < 360);
  assert.match(sign, /name: unsigned-\$\{\{ matrix\.target \}\}/);
  // Release assets keep the names attach-macos and update-homebrew read.
  assert.match(sign, /name: hypercolor-tarball-\$\{\{ steps\.payload\.outputs\.version \}\}-\$\{\{ matrix\.target \}\}/);
  assert.match(sign, /name: hypercolor-app-\$\{\{ steps\.payload\.outputs\.version \}\}-\$\{\{ matrix\.target \}\}-\$\{\{ matrix\.artifact-kind \}\}/);
  assert.match(sign, /path: target\/\$\{\{ matrix\.rust-target \}\}\/release\/bundle\/dmg\/\*\.dmg\*/);
  // The unsigned payload must never reach a release: create-release only
  // downloads artifacts matching its pattern, which unsigned-* cannot match.
  const toGlob = pattern => new RegExp(`^${pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')}$`);
  for (const id of ['create-release', 'attach-macos']) {
    const glob = toGlob(body(id).match(/pattern: (\S+)/)[1]);
    assert.ok(!glob.test('unsigned-macos-arm64'), `${id} must not download unsigned payloads`);
  }
  // The signed macOS artifacts attach-macos downloads are exactly the ones
  // sign-macos uploads, and create-release never publishes macOS assets.
  const attachGlob = toGlob(body('attach-macos').match(/pattern: (\S+)/)[1]);
  assert.ok(attachGlob.test('hypercolor-tarball-0.6.0-macos-arm64'));
  assert.ok(attachGlob.test('hypercolor-app-0.6.0-macos-arm64-dmg'));
  assert.ok(!attachGlob.test('hypercolor-app-0.6.0-windows-x64-nsis'));
  const releaseFilter = body('create-release');
  assert.match(releaseFilter, /! -name '\*-macos-\*'/);
  assert.doesNotMatch(releaseFilter, /-name '\*\.dmg'/);
  // Signing runs exactly when the build it signs runs, except a warm dispatch,
  // which builds unsigned only to seed the release compiler cache.
  const condition = job => job.match(/^    if: >-\n((?:      .*\n)+)/m)[1];
  assert.equal(condition(sign), condition(build).replace(
    `contains(fromJSON('["full","warm"]'), inputs.release_artifacts))`, "inputs.release_artifacts == 'full')"));
  // Apple silicon is the only macOS target in both stages, and no Intel
  // runner or target may return to the release pipeline.
  const targets = job => [...job.matchAll(/^          - target: (\S+)$/gm)].map(match => match[1]);
  assert.deepEqual(targets(build), ['windows-x64', 'macos-arm64']);
  assert.deepEqual(targets(sign), ['macos-arm64']);
  for (const job of [build, sign, body('attach-macos')]) {
    assert.doesNotMatch(job, /macos-x64|macos-amd64|macos-26-intel|rust-target: x86_64-apple-darwin|cask_arch: x86_64/);
  }
});

test('native app version validation accepts an exact stamped prerelease', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const job = workflow.match(/^  build-native-app:\n([\s\S]*?)(?=^  [a-z][\w-]*:)/m)[1];
  const step = job.match(/      - name: Determine version\n([\s\S]*?)(?=      - name:)/)[1];
  assert.match(step, /if \(\$version -ne \$cargoVersion -and \$baseVersion -ne \$cargoVersion\)/);
});

function homebrewJob() {
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
  return { repo, workflow, job, steps, shell };
}

// Run the job's checksum and render steps against a release that carries
// `assets`, with `published` standing in for the tap's current formula.
function runHomebrewJob({ assets, published, failDownloads = [] }) {
  const { repo, steps, shell } = homebrewJob();
  const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-workflow-'));
  const output = path.join(dir, 'outputs');
  const env = { ...process.env, VERSION: '0.5.2', GITHUB_OUTPUT: output, RUNNER_TEMP: dir,
    FIXTURE_ASSETS: assets.join('\n'), FIXTURE_FAILING: failDownloads.join('\n') };
  // Substitute only the release transport; execute the workflow shell itself.
  const gh = `gh() {
    if [[ "$1 $2" == "release view" ]]; then
      printf '%s\\n' "$FIXTURE_ASSETS"
      return 0
    fi
    local artifact='' directory=''
    while (( $# )); do
      case "$1" in
        --pattern) artifact="$2"; shift ;;
        --dir) directory="$2"; shift ;;
      esac
      shift
    done
    test -n "$artifact" && test -n "$directory" || return 99
    grep -qxF "$artifact" <<<"$FIXTURE_ASSETS" || return 98
    if [[ -n "$FIXTURE_FAILING" ]] && grep -qxF "$artifact" <<<"$FIXTURE_FAILING"; then return 1; fi
    printf 'fixture:%s' "$artifact" > "$directory/$artifact"
  }
  `;
  const checksums = spawnSync('bash', ['-c', gh + shell(steps.get('Download release tarballs and compute checksums'))],
    { cwd: dir, env, encoding: 'utf8' });
  const values = checksums.status === 0
    ? Object.fromEntries(readFileSync(output, 'utf8').trim().split('\n').map(line => line.split('=')))
    : {};
  let rendered;
  if (checksums.status === 0) {
    const renderStep = steps.get('Render formula and cask');
    for (const [, name, key] of renderStep.matchAll(/^          (SHA256_\w+): \$\{\{ steps.checksums.outputs.(\w+) \}\}/gm)) {
      env[name] = values[key] ?? '';
    }
    mkdirSync(path.join(dir, 'scripts'));
    copyFileSync(path.join(repo, 'scripts/homebrew-formula.mjs'), path.join(dir, 'scripts/homebrew-formula.mjs'));
    symlinkSync(path.join(repo, 'packaging'), path.join(dir, 'packaging'));
    mkdirSync(path.join(dir, 'homebrew-tap/Formula'), { recursive: true });
    mkdirSync(path.join(dir, 'homebrew-tap/Casks'), { recursive: true });
    writeFileSync(path.join(dir, 'homebrew-tap/Formula/hypercolor.rb'), published.formula);
    writeFileSync(path.join(dir, 'homebrew-tap/Casks/hypercolor-app.rb'), published.cask);
    rendered = spawnSync('bash', ['-c', shell(renderStep)], { cwd: dir, env, encoding: 'utf8' });
  }
  // Run the push step with git stubbed so the commit it would write is visible.
  const push = () => {
    const git = `git() {
      case "$1" in
        diff) return 1 ;;
        commit) shift; printf 'COMMIT'; printf ' [%s]' "$@"; printf '\\n' ;;
      esac
      return 0
    }
    `;
    const pushEnv = { ...env, SHA256_MACOS_ARM64: values.sha256_macos_arm64 ?? '' };
    return spawnSync('bash', ['-c', git + shell(steps.get('Push to homebrew-tap'))],
      { cwd: dir, env: pushEnv, encoding: 'utf8' });
  };
  return { dir, checksums, values, rendered, push,
    read: file => readFileSync(path.join(dir, 'homebrew-tap', file), 'utf8') };
}

const linuxAssets = ['hypercolor-0.5.2-linux-amd64.tar.gz', 'hypercolor-0.5.2-linux-arm64.tar.gz'];
const macosAssets = ['hypercolor-0.5.2-macos-arm64.tar.gz', 'Hypercolor-0.5.2-arm64.dmg'];
const fixtureSha = asset => createHash('sha256').update(`fixture:${asset}`).digest('hex');
const publishedTap = {
  formula: `class Hypercolor < Formula
  version "0.5.1"

  on_macos do
    version "0.3.2"
    url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-arm64.tar.gz"
    sha256 "${'9'.repeat(64)}"
  end
end
`,
  cask: 'cask "hypercolor-app" do\n  version "0.3.2"\nend\n',
};

test('Homebrew checksum step supplies every value consumed by its renderer', () => {
  const run = runHomebrewJob({ assets: [...linuxAssets, ...macosAssets], published: publishedTap });
  try {
    assert.equal(run.checksums.status, 0, run.checksums.stderr);
    const expectedAssets = {
      sha256_linux_amd64: linuxAssets[0],
      sha256_linux_arm64: linuxAssets[1],
      sha256_macos_arm64: macosAssets[0],
      sha256_dmg_arm64: macosAssets[1],
    };
    assert.deepEqual(Object.keys(run.values).sort(), Object.keys(expectedAssets).sort());
    for (const [key, asset] of Object.entries(expectedAssets)) {
      assert.equal(run.values[key], fixtureSha(asset));
    }
    assert.equal(run.rendered.status, 0, run.rendered.stderr);
    for (const file of ['Formula/hypercolor.rb', 'Casks/hypercolor-app.rb']) {
      const content = run.read(file);
      assert.match(content, /version "0\.5\.2"/);
      assert.doesNotMatch(content, /version "0\.3\.2"|PLACEHOLDER|SHA256_/);
    }
    const pushed = run.push();
    assert.equal(pushed.status, 0, pushed.stderr);
    assert.match(pushed.stdout, /COMMIT \[-m\] \[hypercolor: update to 0\.5\.2\] \[-m\] \[Update formula and cask/);
    // Homebrew never asks the release for an Intel macOS artifact.
    const { job, workflow } = homebrewJob();
    assert.doesNotMatch(job, /macos-amd64|x86_64|MACOS_AMD64/i);
    const aur = workflow.match(/^  update-aur:\n([\s\S]*?)(?=^  [a-z][\w-]*:)/m)[1];
    assert.doesNotMatch(aur, /macos-|\.dmg/);
  } finally {
    rmSync(run.dir, { recursive: true, force: true });
  }
});

test('a release without a notarized macOS build advances Linux and carries macOS forward', () => {
  const run = runHomebrewJob({ assets: linuxAssets, published: publishedTap });
  try {
    assert.equal(run.checksums.status, 0, run.checksums.stderr);
    assert.match(run.checksums.stdout, /no signed macOS build; macOS stays on the published formula/);
    assert.deepEqual(Object.keys(run.values).sort(), ['sha256_linux_amd64', 'sha256_linux_arm64']);
    assert.equal(run.rendered.status, 0, run.rendered.stderr);
    const formula = run.read('Formula/hypercolor.rb');
    assert.match(formula, /^  version "0\.5\.2"$/m);
    const macDownload = formula.match(/^  on_macos do\n([\s\S]*?)^  end\n/m)[1];
    assert.match(macDownload, /^    version "0\.3\.2"$/m);
    assert.match(macDownload, new RegExp(`sha256 "${'9'.repeat(64)}"`));
    assert.match(formula, new RegExp(`sha256 "${fixtureSha(linuxAssets[0])}"`));
    assert.equal(run.read('Casks/hypercolor-app.rb'), publishedTap.cask);
    const pushed = run.push();
    assert.equal(pushed.status, 0, pushed.stderr);
    assert.match(pushed.stdout, /\[Linux moves to 0\.5\.2; macOS and the cask stay as published until a notarized build ships\.\]/);
  } finally {
    rmSync(run.dir, { recursive: true, force: true });
  }
});

test('a withdrawn macOS stanza stays withdrawn while Linux advances', () => {
  const template = readFileSync(new URL('../../packaging/homebrew/hypercolor.rb', import.meta.url), 'utf8');
  const withdrawn = renderFormula({ template, version: '0.5.1',
    linux: { amd64: '1'.repeat(64), arm64: '2'.repeat(64) }, macos: { withdrawn: true } });
  const run = runHomebrewJob({ assets: linuxAssets, published: { formula: withdrawn, cask: publishedTap.cask } });
  try {
    assert.equal(run.checksums.status, 0, run.checksums.stderr);
    assert.equal(run.rendered.status, 0, run.rendered.stderr);
    assert.match(run.rendered.stdout, /macOS stays withdrawn and the cask is unchanged/);
    const formula = run.read('Formula/hypercolor.rb');
    assert.match(formula, /^  version "0\.5\.2"$/m);
    const macDownload = formula.match(/^  on_macos do\n([\s\S]*?)^  end\n/m)[1];
    assert.match(macDownload, /depends_on NotarizedMacosBuildRequirement/);
    assert.match(macDownload, new RegExp(`sha256 "${fixtureSha(linuxAssets[0])}"`));
    assert.equal(run.read('Casks/hypercolor-app.rb'), publishedTap.cask);
  } finally {
    rmSync(run.dir, { recursive: true, force: true });
  }
});

test('a failed macOS download stops the job instead of carrying macOS forward', () => {
  for (const failing of macosAssets) {
    const run = runHomebrewJob({ assets: [...linuxAssets, ...macosAssets], published: publishedTap,
      failDownloads: [failing] });
    try {
      assert.notEqual(run.checksums.status, 0, `${failing} download failure must stop the step`);
      assert.equal(run.rendered, undefined);
    } finally {
      rmSync(run.dir, { recursive: true, force: true });
    }
  }
});

test('a release with only part of its macOS assets never reaches the renderer', () => {
  for (const partial of macosAssets) {
    const run = runHomebrewJob({ assets: [...linuxAssets, partial], published: publishedTap });
    try {
      assert.equal(run.checksums.status, 1);
      assert.match(run.checksums.stdout, /carries only part of its macOS assets/);
      assert.equal(run.rendered, undefined);
    } finally {
      rmSync(run.dir, { recursive: true, force: true });
    }
  }
});

test('the unsigned payload carries the build job binaries through signing into the tarball', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const body = id => workflow.match(new RegExp(`^  ${id}:\\n([\\s\\S]*?)(?=^  [a-z][\\w-]*:|^  # ──)`, 'm'))[1];
  const native = body('build-native-app');
  const sign = body('sign-macos');
  const tarballs = body('build-release');
  assert.doesNotMatch(tarballs, /target: macos-/);
  const script = (job, name) => {
    const step = job.match(new RegExp(`      - name: ${name}\\n([\\s\\S]*?)(?=      - name:|      - uses:|$)`))[1];
    return step.split('        run: |\n')[1].replace(/^          /gm, '');
  };
  const pack = script(native, 'Package unsigned macOS payload');
  const unpack = script(sign, 'Unpack unsigned macOS payload');
  const assemble = script(sign, 'Assemble signed macOS distribution');
  assert.doesNotMatch(assemble, /cargo build|cargo tauri|CARGO_TARGET_DIR/);
  assert.match(sign, /dist\/hypercolor-\*\.tar.gz\n/);
  for (const [arch, target, matrixTarget, platform] of [
    ['arm64', 'aarch64-apple-darwin', 'macos-arm64', 'macos-arm64'],
  ]) {
    const buildRunner = mkdtempSync(path.join(tmpdir(), 'macos-build-runner-'));
    const signRunner = mkdtempSync(path.join(tmpdir(), 'macos-sign-runner-'));
    try {
      // Build runner: the sidecars in the host profile, the app host and the
      // unsigned bundle under the explicit target, as the Tauri build leaves them.
      const targetDir = path.join(buildRunner, 'target');
      mkdirSync(path.join(targetDir, 'release'), { recursive: true });
      const bundleMacos = path.join(targetDir, target, 'release/bundle/macos');
      mkdirSync(path.join(bundleMacos, 'Hypercolor.app/Contents/MacOS'), { recursive: true });
      for (const binary of ['hypercolor-daemon', 'hypercolor']) {
        writeFileSync(path.join(targetDir, 'release', binary), binary, { mode: 0o755 });
      }
      writeFileSync(path.join(targetDir, target, 'release/hypercolor-app'), 'hypercolor-app', { mode: 0o755 });
      writeFileSync(path.join(bundleMacos, 'Hypercolor.app/Contents/MacOS/hypercolor-app'), 'bundle-main', { mode: 0o755 });
      writeFileSync(path.join(bundleMacos, 'Hypercolor.app/Contents/Info.plist'), 'plist', { mode: 0o644 });
      const substitute = text => text.replaceAll('${{ matrix.rust-target }}', target)
        .replaceAll('${{ matrix.target }}', matrixTarget)
        .replaceAll('${{ matrix.cask_arch }}', arch)
        .replaceAll('${{ steps.version.outputs.version }}', '0.5.2');
      const packed = spawnSync('bash', ['-e', '-c', substitute(pack)], {
        cwd: buildRunner, encoding: 'utf8',
        env: { ...environment, RUNNER_TEMP: buildRunner, CARGO_TARGET_DIR: targetDir },
      });
      assert.equal(packed.status, 0, packed.stderr);

      // The artifact store hands the tar to a fresh signing runner.
      mkdirSync(path.join(signRunner, 'unsigned'));
      copyFileSync(path.join(buildRunner, `unsigned-${matrixTarget}.tar`),
        path.join(signRunner, 'unsigned', `unsigned-${matrixTarget}.tar`));
      const outputs = path.join(signRunner, 'github-output');
      const unpacked = spawnSync('bash', ['-e', '-c', substitute(unpack)], {
        cwd: signRunner, encoding: 'utf8',
        env: { ...environment, RUNNER_TEMP: signRunner, GITHUB_OUTPUT: outputs },
      });
      assert.equal(unpacked.status, 0, unpacked.stderr);
      const values = Object.fromEntries(readFileSync(outputs, 'utf8').trim().split('\n').map(line => line.split('=')));
      assert.equal(values.version, '0.5.2');
      const restored = path.join(signRunner, 'target', target, 'release/bundle/macos/Hypercolor.app');
      assert.equal(readFileSync(path.join(restored, 'Contents/MacOS/hypercolor-app'), 'utf8'), 'bundle-main');
      assert.equal(statSync(path.join(restored, 'Contents/MacOS/hypercolor-app')).mode & 0o777, 0o755);
      assert.equal(statSync(path.join(restored, 'Contents/Info.plist')).mode & 0o777, 0o644);

      mkdirSync(path.join(signRunner, 'scripts'));
      writeFileSync(path.join(signRunner, 'scripts/with-macos-signing.sh'), '#!/bin/bash\nexec "$@"\n');
      writeFileSync(path.join(signRunner, 'scripts/dist.sh'), `#!/bin/bash
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
case "$bin_dir" in /*) ;; *) exit 64 ;; esac
for binary in hypercolor-daemon hypercolor hypercolor-app; do
  test -x "$bin_dir/$binary"
  test "$(cat "$bin_dir/$binary")" = "$binary"
done
mkdir -p dist/hypercolor-0.5.2-${platform}
printf 'archive-fixture' > dist/hypercolor-0.5.2-${platform}.tar.gz
`, { mode: 0o755 });
      writeFileSync(path.join(signRunner, 'scripts/sign-macos-artifacts.sh'), `#!/bin/bash
set -euo pipefail
test "$1" = verify-standalone
test "$3" = dist/hypercolor-0.5.2-${platform}
test "$5" = '${target}'
test "$7" = fixture-team
`, { mode: 0o755 });
      const run = substitute(assemble)
        .replaceAll('${{ steps.payload.outputs.version }}', values.version)
        .replaceAll('${{ steps.payload.outputs.bin-dir }}', values['bin-dir']);
      const assembled = spawnSync('bash', ['-e', '-c', run], {
        cwd: signRunner, encoding: 'utf8',
        env: { ...environment, RUNNER_TEMP: signRunner, APPLE_TEAM_ID: 'fixture-team' },
      });
      assert.equal(assembled.status, 0, assembled.stderr);
      const checksum = readFileSync(path.join(signRunner, `dist/hypercolor-0.5.2-${platform}.tar.gz.sha256`), 'utf8');
      assert.equal(checksum.trim(), `${createHash('sha256').update('archive-fixture').digest('hex')}  hypercolor-0.5.2-${platform}.tar.gz`);
    } finally {
      rmSync(buildRunner, { recursive: true, force: true });
      rmSync(signRunner, { recursive: true, force: true });
    }
  }
});

test('attach-macos adds only the signed macOS assets the release lacks', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const job = workflow.match(/^  attach-macos:\n([\s\S]*?)(?=^  [a-z][\w-]*:|^  # ──)/m)?.[1];
  assert.ok(job, 'attach-macos job exists');
  assert.match(job, /^      contents: write$/m);
  const step = job.match(/      - name: Attach signed macOS artifacts to the release\n([\s\S]*)/)?.[1];
  const run = step?.match(/^        run: \|\n([\s\S]*)/m)?.[1];
  assert.ok(run, 'attach step has a shell body');
  const shell = run.replace(/^          /gm, '');
  const assets = [
    'hypercolor-0.6.0-macos-arm64.tar.gz', 'hypercolor-0.6.0-macos-arm64.tar.gz.sha256',
    'Hypercolor-0.6.0-arm64.dmg', 'Hypercolor-0.6.0-arm64.dmg.notarization.json',
  ];
  // Substitute only the release transport: `view` lists the published
  // asset names, and `upload` records what it would publish.
  const gh = `gh() {
    case "$1 $2" in
      'release view') cat "$RELEASE_STATE" ;;
      'release upload')
        shift 2; shift; shift 2
        for file in "$@"; do basename "$file" | tee -a "$RELEASE_STATE" >> "$UPLOAD_LOG"; done ;;
      *) return 99 ;;
    esac
  }
  `;
  const run_attach = (published, artifacts = assets) => {
    const dir = mkdtempSync(path.join(tmpdir(), 'attach-macos-'));
    try {
      mkdirSync(path.join(dir, 'macos-artifacts'));
      for (const name of artifacts) writeFileSync(path.join(dir, 'macos-artifacts', name), name);
      // A Linux asset already on the release must never be re-uploaded.
      writeFileSync(path.join(dir, 'state'), ['hypercolor-0.6.0-linux-amd64.tar.gz', ...published].join('\n') + '\n');
      writeFileSync(path.join(dir, 'uploads'), '');
      const result = spawnSync('bash', ['-c', gh + shell], {
        cwd: dir,
        env: { ...process.env, GITHUB_REF_NAME: 'v0.6.0', REPOSITORY: 'hyperb1iss/hypercolor',
          RELEASE_STATE: path.join(dir, 'state'), UPLOAD_LOG: path.join(dir, 'uploads') },
        encoding: 'utf8',
      });
      const uploads = readFileSync(path.join(dir, 'uploads'), 'utf8').split('\n').filter(Boolean).sort();
      return { status: result.status, stdout: result.stdout, stderr: result.stderr, uploads };
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  };

  const first = run_attach([]);
  assert.equal(first.status, 0, first.stderr);
  assert.deepEqual(first.uploads, [...assets].sort());

  const rerun = run_attach(assets);
  assert.equal(rerun.status, 0, rerun.stderr);
  assert.deepEqual(rerun.uploads, []);
  assert.match(rerun.stdout, /already carries every macOS artifact/);

  const partial = run_attach(assets.slice(0, 2));
  assert.equal(partial.status, 0, partial.stderr);
  assert.deepEqual(partial.uploads, assets.slice(2).sort());

  const incomplete = run_attach([], assets.slice(0, 3));
  assert.equal(incomplete.status, 1);
  assert.match(incomplete.stderr, /expected exactly the signed Apple silicon macOS artifacts/);
  assert.match(incomplete.stderr, /found 3:/);
  assert.deepEqual(incomplete.uploads, []);

  // An Intel artifact is never published, even beside a complete arm64 set.
  const intel = run_attach([], [...assets, 'Hypercolor-0.6.0-x86_64.dmg']);
  assert.equal(intel.status, 1);
  assert.match(intel.stderr, /Hypercolor-0\.6\.0-x86_64\.dmg/);
  assert.deepEqual(intel.uploads, []);

  // Assets from another version never stand in for this tag's.
  const stale = run_attach([], assets.map(name => name.replace('0.6.0', '0.5.9')));
  assert.equal(stale.status, 1);
  assert.deepEqual(stale.uploads, []);
});
