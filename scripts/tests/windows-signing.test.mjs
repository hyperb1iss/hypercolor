import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8');
const workflow = read('../../.github/workflows/ci.yml');
const selftest = read('../../.github/workflows/windows-signing-selftest.yml');
const release = read('../../.github/workflows/release.yml');
const installer = read('../../crates/hypercolor-app/installer.nsi');
const signer = read('../sign-windows-artifacts.ps1');

const jobsOf = (source) => new Map([...source.slice(source.indexOf('\njobs:')).matchAll(
  /^ {2}([a-z][\w-]*):\n([\s\S]*?)(?=^ {2}[a-z][\w-]*:\n|(?![\s\S]))/gm)].map(([, name, body]) => [name, body]));
const jobs = jobsOf(workflow);
const job = (name) => {
  assert.ok(jobs.has(name), `missing job ${name}`);
  return jobs.get(name);
};
const steps = (body) => body.split(/\n(?= {6}- )/).filter((chunk) => /^ {6}- /.test(chunk)).map((text) => ({
  text,
  name: text.match(/^ {6}- name: (.+)$/m)?.[1] ?? text.match(/^ {6}- uses: (\S+)/m)?.[1],
  uses: text.match(/^ {6}(?:- | {2})uses: (\S+)/m)?.[1],
  condition: text.match(/^ {8}if: (.+)$/m)?.[1],
}));
const step = (body, name) => {
  const found = steps(body).find((candidate) => candidate.name === name);
  assert.ok(found, `missing step ${name}`);
  return found;
};

// Evaluates the GitHub expression subset these conditions use, with the
// release-credentials decision and the runner OS as the only inputs.
function evaluate(expression, { signing, os = 'Windows' }) {
  const source = expression.replace(/^\$\{\{\s*|\s*\}\}$/g, '')
    .replace(/\bneeds\.([\w-]+)/g, "needs['$1']");
  const needs = { 'release-credentials': { outputs: { windows_signing: signing } } };
  return Function('needs', 'runner', `return (${source});`)(needs, { os });
}

const configNames = [...signer.match(/^\$ConfigNames = @\(\n((?: {4}'[A-Z_]+',?\n)+)\)/m)[1]
  .matchAll(/'([A-Z_]+)'/g)].map((match) => match[1]);

test('the build job never holds an OIDC token or signing configuration', () => {
  const build = job('build-native-app');
  assert.doesNotMatch(build, /id-token|azure\/login|vars\.(AZURE|ARTIFACT_SIGNING|WINDOWS_SIGNING)/);
  // The unsigned path is today's path: the Tauri build never signs.
  const bundle = step(build, 'Build Tauri native bundle').text;
  assert.match(bundle, /^ {10}\$buildArgs \+= "--no-sign"$/m);
  assert.doesNotMatch(bundle, /signCommand|HYPERCOLOR_TAURI_SIGN_CONFIG/);
});

test('only sign-windows and the publishing jobs can mint OIDC tokens', () => {
  const minting = [...jobs].filter(([, body]) => /^ {6}id-token: write/m.test(body)).map(([name]) => name);
  assert.deepEqual(minting.sort(), ['docs-deploy', 'publish-npm', 'publish-pypi', 'sign-windows']);
});

test('release-credentials decides signing once for every Windows job', () => {
  const credentials = job('release-credentials');
  assert.match(credentials, /^ {6}windows_signing: \$\{\{ steps\.windows-signing\.outputs\.enabled \}\}$/m);
  const resolve = step(credentials, 'Resolve Windows signing').text;
  assert.match(resolve, /run: \.\/scripts\/sign-windows-artifacts\.ps1 resolve/);
  assert.match(resolve, /RELEASE_ARTIFACTS: \$\{\{ inputs\.release_artifacts \}\}/);
  assert.equal(configNames.length, 7);
  const setup = step(job('sign-windows'), 'Prepare Artifact Signing').text;
  const releaseJob = jobsOf(release).get('release');
  const consumers = [
    ['release-credentials', resolve],
    ['sign-windows setup', setup],
    ['selftest resolve', step(jobsOf(selftest).get('selftest'), 'Resolve Windows signing').text],
    ['selftest setup', step(jobsOf(selftest).get('selftest'), 'Prepare Artifact Signing').text],
    ['release preflight', step(releaseJob, 'Check Windows signing configuration').text],
  ];
  for (const [where, text] of consumers) {
    for (const name of configNames) {
      assert.ok(text.includes(`${name}: \${{ vars.${name} }}`), `${where} must pass ${name}`);
    }
  }
  assert.match(step(releaseJob, 'Check Windows signing configuration').text,
    /SIGNING_REF: refs\/tags\/v\$\{\{ inputs\.version \}\}/);
});

test('exactly one job uploads the Windows release installer', () => {
  const build = job('build-native-app');
  const sign = job('sign-windows');
  const installerUploads = (body) => steps(body)
    .filter((candidate) => candidate.uses?.startsWith('actions/upload-artifact@') && /name: hypercolor-app-/.test(candidate.text));
  for (const signing of ['true', 'false']) {
    const fromBuild = installerUploads(build)
      .filter((candidate) => candidate.text.includes('windows') || candidate.condition?.includes("'Windows'"))
      .filter((candidate) => evaluate(candidate.condition, { signing }));
    const fromSign = installerUploads(sign).filter((candidate) => evaluate(candidate.condition, { signing }));
    assert.equal(fromBuild.length + fromSign.length, 1, `signing=${signing}`);
    assert.equal(fromSign.length, signing === 'true' ? 1 : 0, `signing=${signing}`);
    const payload = step(build, 'Upload unsigned Windows payload');
    assert.equal(Boolean(evaluate(payload.condition, { signing })), signing === 'true');
  }
  // create-release downloads hypercolor-*, which the unsigned payload never matches.
  const pattern = job('create-release').match(/pattern: (\S+)/)[1];
  const glob = new RegExp(`^${pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')}$`);
  assert.ok(!glob.test('unsigned-windows-x64'));
  assert.ok(glob.test('hypercolor-app-1.0.0-windows-x64-nsis'));
  assert.match(step(build, 'Upload unsigned Windows payload').text, /name: unsigned-\$\{\{ matrix\.target \}\}/);
  assert.match(step(sign, 'Download unsigned Windows payload').text, /name: unsigned-windows-x64/);
});

test('sign-windows follows the signed release lanes and gates publication', () => {
  const sign = job('sign-windows');
  const condition = (body) => body.match(/^ {4}if: >-\n((?: {6}.*\n)+)/m)[1];
  assert.equal(condition(sign), condition(job('sign-macos')));
  assert.match(sign, /^ {4}needs: \[build-native-app, release-credentials\]$/m);
  assert.match(job('create-release'), /^ {4}needs: \[build-release, build-native-app, sign-windows, /m);
  const runsOn = sign.match(/^ {4}runs-on: (.+)$/m)[1];
  const environment = sign.match(/^ {4}environment:\n(?: {6}#.*\n)* {6}name: (.+)$/m)[1];
  for (const signing of ['true', 'false']) {
    assert.equal(evaluate(runsOn, { signing }), signing === 'true' ? 'windows-latest' : 'ubuntu-24.04');
    assert.equal(evaluate(environment, { signing }), signing === 'true' ? 'windows-signing' : '');
  }
});

test('every signing step waits for the decision, and setup follows the login', () => {
  const all = steps(job('sign-windows'));
  const [report, ...rest] = all;
  assert.equal(report.name, 'Report an unsigned Windows build');
  assert.equal(evaluate(report.condition, { signing: 'false' }), true);
  assert.equal(evaluate(report.condition, { signing: 'true' }), false);
  for (const candidate of rest) {
    assert.equal(candidate.condition, "needs.release-credentials.outputs.windows_signing == 'true'", candidate.name);
  }
  const order = all.map((candidate) => candidate.name);
  const login = order.indexOf('Sign in to Azure');
  assert.ok(login > order.indexOf('Restore unsigned Windows payload'));
  assert.equal(order[login + 1], 'Prepare Artifact Signing');
  assert.equal(order[login + 2], 'Bundle and sign Windows installer');
  assert.equal(order[login + 3], 'Verify Windows signatures');
  assert.equal(order.at(-1), 'Upload native release bundle');
  // The signing job installs Tauri's own pinned release binary, the same
  // version build-native-app pins, and compiles nothing.
  const install = step(job('sign-windows'), 'Install Tauri CLI').text;
  const pinned = job('build-native-app').match(/"tauri-cli@=([\d.]+)"/)[1];
  assert.match(install, new RegExp(`TAURI_CLI_VERSION: "${pinned.replaceAll('.', '\\.')}"`));
  assert.match(install, /TAURI_CLI_SHA256: [0-9a-f]{64}$/m);
  assert.match(install, /Get-FileHash -Algorithm SHA256/);
  assert.doesNotMatch(job('sign-windows'), /cargo (build|install|binstall)|cargo-cache-build|install-action/);
  const bundle = step(job('sign-windows'), 'Bundle and sign Windows installer').text;
  assert.match(bundle, /cargo tauri bundle --ci --bundles nsis --no-binary-patching/);
  assert.match(bundle, /--config \$env:HYPERCOLOR_TAURI_SIGN_CONFIG/);
  assert.doesNotMatch(bundle, /--no-sign/);
  const verify = step(job('sign-windows'), 'Verify Windows signatures').text;
  assert.match(verify, /sign-windows-artifacts\.ps1 verify -Installer .+ -UnsignedInstaller/);
  assert.match(verify, /check-windows-dll-closure\.ps1 -Installer/);
});

test('every action in the jobs that sign is pinned to a commit', () => {
  const signers = [['sign-windows', job('sign-windows')], ['selftest', jobsOf(selftest).get('selftest')]];
  for (const [name, body] of signers) {
    assert.match(body, /^ {6}id-token: write$/m, `${name} is a signing job`);
    const actions = [...body.matchAll(/uses: (\S+)/g)].map((match) => match[1]);
    assert.ok(actions.includes('azure/login@a641126d1b8aa4d1fa005f4f92df94a3a4c4c906'), name);
    for (const action of actions) {
      assert.match(action, /^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/, `${name} uses ${action}`);
    }
  }
});

test('a failed uninstaller signature fails makensis', () => {
  assert.match(installer, /^ {2}!uninstfinalize '\$\{UNINSTALLERSIGNCOMMAND\}' = 0$/m);
  assert.doesNotMatch(installer, /!uninstfinalize '\$\{UNINSTALLERSIGNCOMMAND\}'$/m);
});

test('signing changes run the workflow lane', () => {
  const filter = job('changes').match(/^ {12}workflow:\n((?: {14}- .+\n)+)/m)[1];
  for (const path of ['scripts/sign-windows-artifacts.ps1', 'scripts/tests/windows-signing-tests.ps1',
    'scripts/tests/windows-signing.test.mjs']) {
    assert.ok(filter.includes(`- '${path}'`), path);
  }
  const lint = job('workflow-lint');
  assert.match(lint, /scripts\/tests\/windows-signing\.test\.mjs/);
  assert.match(lint, /run: \.\/scripts\/tests\/windows-signing-tests\.ps1/);
});
