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

const workflowText = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');

function jobSteps(id) {
  const job = workflowText.match(new RegExp(`^  ${id}:\\n([\\s\\S]*?)(?=^  [a-z][\\w-]*:|$(?![\\s\\S]))`, 'm'))?.[1];
  assert.ok(job, `${id} job exists`);
  return new Map([...job.matchAll(/^      - name: (.+)\n([\s\S]*?)(?=^      - |$(?![\s\S]))/gm)]
    .map(([, name, body]) => [name, body]));
}

function shell(body) {
  assert.ok(body, 'expected workflow step exists');
  const run = body.match(/^        run: \|\n([\s\S]*)/m)?.[1];
  assert.ok(run, 'step has a shell body');
  return run.replace(/^          /gm, '').replaceAll('${{ github.repository }}', 'hyperb1iss/hypercolor');
}

const readOutputs = file => Object.fromEntries(readFileSync(file, 'utf8').trim().split('\n')
  .filter(Boolean).map(line => [line.slice(0, line.indexOf('=')), line.slice(line.indexOf('=') + 1)]));

const appleSecrets = ['APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_SIGNING_IDENTITY',
  'APPLE_TEAM_ID', 'APPLE_API_KEY_ID', 'APPLE_API_ISSUER', 'APPLE_API_KEY_CONTENT'];

test('credential probe drops the macOS lanes when any Apple secret is missing', () => {
  const repo = fileURLToPath(new URL('../../', import.meta.url));
  const probe = shell(jobSteps('release-credentials').get('Probe signing credentials and select release lanes'));
  const matrices = JSON.parse(readFileSync(path.join(repo, '.github/release-matrix.json'), 'utf8'));
  assert.ok(matrices.native.some(entry => entry.signing) && matrices.release.some(entry => entry.signing));
  assert.ok(matrices.native.some(entry => !entry.signing) && matrices.release.some(entry => !entry.signing));
  const dir = mkdtempSync(path.join(tmpdir(), 'release-credentials-'));
  try {
    const run = (env) => {
      const output = path.join(dir, `outputs-${Math.random()}`);
      const result = spawnSync('bash', ['-c', probe], { cwd: repo, encoding: 'utf8',
        env: { ...process.env, ...env, GITHUB_OUTPUT: output } });
      assert.equal(result.status, 0, result.stderr);
      return { result, outputs: readOutputs(output) };
    };
    const complete = run(Object.fromEntries(appleSecrets.map(name => [name, 'fixture-only'])));
    assert.equal(complete.outputs.macos, 'true');
    assert.doesNotMatch(complete.result.stdout, /::warning::/);
    for (const lane of ['native', 'release']) {
      const selected = JSON.parse(complete.outputs[`${lane}_matrix`]);
      assert.deepEqual(selected.map(entry => entry.target), matrices[lane].map(entry => entry.target));
      assert.ok(selected.every(entry => !('signing' in entry)), 'the selector key never reaches the matrix');
    }
    const missing = run(Object.fromEntries(appleSecrets.map(name => [name, name === 'APPLE_TEAM_ID' ? '' : 'fixture-only'])));
    assert.equal(missing.outputs.macos, 'false');
    assert.match(missing.result.stdout, /^::warning::.*missing APPLE_TEAM_ID\b.*Linux and Windows only/m);
    for (const lane of ['native', 'release']) {
      const selected = JSON.parse(missing.outputs[`${lane}_matrix`]);
      assert.deepEqual(selected.map(entry => entry.target),
        matrices[lane].filter(entry => !entry.signing).map(entry => entry.target));
      assert.ok(selected.length > 0, `${lane} still builds its unsigned platforms`);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  for (const [id, output] of [['build-native-app', 'native-matrix'], ['build-release', 'release-matrix']]) {
    const job = workflowText.match(new RegExp(`^  ${id}:\\n([\\s\\S]*?)(?=^  [a-z][\\w-]*:)`, 'm'))[1];
    assert.ok(job.includes(`include: \${{ fromJSON(needs.release-credentials.outputs.${output}) }}`), `${id} takes its matrix from the probe`);
  }
});

test('Homebrew step tracks macOS only when the release published the whole signed set', () => {
  const repo = fileURLToPath(new URL('../../', import.meta.url));
  const steps = jobSteps('update-homebrew');
  const checksums = shell(steps.get('Download release tarballs and compute checksums'));
  const renderStep = steps.get('Render formula and cask');
  const render = shell(renderStep);
  const sha = seed => seed.repeat(64);
  // The tap before this release: Linux at 0.5.1, macOS carried at 0.3.2.
  const currentFormula = `class Hypercolor < Formula
  version "0.5.1"

  on_macos do
    version "0.3.2"
    if Hardware::CPU.arm?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-arm64.tar.gz"
      sha256 "${sha('c')}"
    end
  end
end
`;
  const currentCask = 'cask "hypercolor-app" do\n  version "0.3.2"\nend\n';
  const linuxAssets = ['hypercolor-0.5.2-linux-amd64.tar.gz', 'hypercolor-0.5.2-linux-arm64.tar.gz'];
  const macosAssets = ['hypercolor-0.5.2-macos-amd64.tar.gz', 'hypercolor-0.5.2-macos-arm64.tar.gz',
    'Hypercolor-0.5.2-arm64.dmg', 'Hypercolor-0.5.2-x86_64.dmg'];
  // Substitute only the GitHub transport; execute the workflow shell itself.
  const gh = `gh() {
      if [[ "$1 $2" == "release view" ]]; then printf '%s\\n' $ASSETS; return 0; fi
      local artifact='' directory=''
      while (( $# )); do
        case "$1" in
          --pattern) artifact="$2"; shift ;;
          --dir) directory="$2"; shift ;;
        esac
        shift
      done
      test -n "$artifact" && test -n "$directory" || return 99
      grep -qxF "$artifact" <<<"$(printf '%s\\n' $ASSETS)" || return 98
      printf 'fixture:%s' "$artifact" > "$directory/$artifact"
    }
    `;
  const scenario = (assets, check) => {
    const dir = mkdtempSync(path.join(tmpdir(), 'homebrew-workflow-'));
    try {
      const output = path.join(dir, 'outputs');
      const env = { ...process.env, VERSION: '0.5.2', GITHUB_OUTPUT: output, ASSETS: assets.join(' ') };
      mkdirSync(path.join(dir, 'homebrew-tap/Formula'), { recursive: true });
      mkdirSync(path.join(dir, 'homebrew-tap/Casks'), { recursive: true });
      writeFileSync(path.join(dir, 'homebrew-tap/Formula/hypercolor.rb'), currentFormula);
      writeFileSync(path.join(dir, 'homebrew-tap/Casks/hypercolor-app.rb'), currentCask);
      mkdirSync(path.join(dir, 'scripts'));
      copyFileSync(path.join(repo, 'scripts/homebrew-formula.mjs'), path.join(dir, 'scripts/homebrew-formula.mjs'));
      symlinkSync(path.join(repo, 'packaging'), path.join(dir, 'packaging'));
      const downloaded = spawnSync('bash', ['-c', gh + checksums], { cwd: dir, env, encoding: 'utf8' });
      check(downloaded, () => {
        const values = readOutputs(output);
        // GitHub materialises every env line, so an unset output arrives as
        // an empty string rather than an unbound variable.
        for (const [, name, key] of renderStep.matchAll(/^          (SHA256_\w+): \$\{\{ steps.checksums.outputs.(\w+) \}\}/gm)) {
          env[name] = values[key] ?? '';
        }
        env.MACOS_PUBLISHED = values.macos;
        const rendered = spawnSync('bash', ['-c', render], { cwd: dir, env, encoding: 'utf8' });
        assert.equal(rendered.status, 0, rendered.stderr);
        return { values, formula: readFileSync(path.join(dir, 'homebrew-tap/Formula/hypercolor.rb'), 'utf8'),
          cask: readFileSync(path.join(dir, 'homebrew-tap/Casks/hypercolor-app.rb'), 'utf8') };
      });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  };
  const digest = asset => createHash('sha256').update(`fixture:${asset}`).digest('hex');

  scenario([...linuxAssets, ...macosAssets], (downloaded, renderTap) => {
    assert.equal(downloaded.status, 0, downloaded.stderr);
    const { values, formula, cask } = renderTap();
    assert.equal(values.macos, 'true');
    assert.deepEqual(Object.keys(values).sort(), ['macos', 'sha256_dmg_arm64', 'sha256_dmg_x86_64',
      'sha256_linux_amd64', 'sha256_linux_arm64', 'sha256_macos_amd64', 'sha256_macos_arm64']);
    assert.equal(values.sha256_macos_arm64, digest('hypercolor-0.5.2-macos-arm64.tar.gz'));
    assert.equal(values.sha256_dmg_x86_64, digest('Hypercolor-0.5.2-x86_64.dmg'));
    assert.deepEqual([...formula.matchAll(/version "([^"]+)"/g)].map(match => match[1]), ['0.5.2', '0.5.2']);
    assert.match(cask, /version "0\.5\.2"/);
    for (const file of [formula, cask]) assert.doesNotMatch(file, /PLACEHOLDER|SHA256_/);
  });

  scenario(linuxAssets, (downloaded, renderTap) => {
    assert.equal(downloaded.status, 0, downloaded.stderr);
    assert.match(downloaded.stdout, /keeps its current macOS build/);
    const { values, formula, cask } = renderTap();
    assert.equal(values.macos, 'false');
    assert.deepEqual(Object.keys(values).sort(), ['macos', 'sha256_linux_amd64', 'sha256_linux_arm64']);
    assert.equal(values.sha256_linux_amd64, digest('hypercolor-0.5.2-linux-amd64.tar.gz'));
    assert.match(formula, /^  version "0\.5\.2"$/m);
    assert.match(formula, /^    version "0\.3\.2"$/m);
    assert.ok(formula.includes(`macos-arm64.tar.gz"\n      sha256 "${sha('c')}"`), 'macOS stanza carried forward');
    assert.ok(!formula.includes('macos-amd64'));
    assert.equal(cask, currentCask, 'the cask is untouched without a new DMG');
  });

  scenario([...linuxAssets, macosAssets[0], macosAssets[2]], (downloaded) => {
    assert.equal(downloaded.status, 1);
    assert.match(downloaded.stdout + downloaded.stderr, /published 2 of 4 macOS artifacts; refusing to advance the tap/);
  });

  const aur = workflowText.match(/^  update-aur:\n([\s\S]*?)(?=^  [a-z][\w-]*:)/m)[1];
  assert.doesNotMatch(aur, /macos-|\.dmg/);
});
