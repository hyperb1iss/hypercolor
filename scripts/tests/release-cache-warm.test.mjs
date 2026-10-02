import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
const scheduler = readFileSync(new URL('../../.github/workflows/release-cache-warm.yml', import.meta.url), 'utf8');
const jobs = new Map([...workflow.slice(workflow.indexOf('\njobs:')).matchAll(/^  ([a-z][\w-]*):\n([\s\S]*?)(?=^  [a-z][\w-]*:\n|(?![\s\S]))/gm)]
  .map(([, name, body]) => [name, body]));

// Evaluates the subset of the GitHub expression language these conditions use.
function evaluate(expression, { ref, event, mode = '' }) {
  const context = {
    github: { ref, event_name: event },
    inputs: { release_artifacts: mode },
    steps: { filter: { outputs: new Proxy({}, { get: () => 'filtered' }) } },
  };
  const source = expression.replace(/^\$\{\{\s*|\s*\}\}$/g, '');
  return Function('github', 'inputs', 'steps', 'contains', 'fromJSON', 'startsWith', `return (${source});`)(
    context.github, context.inputs, context.steps,
    (list, value) => list.includes(value), JSON.parse, (value, prefix) => value.startsWith(prefix),
  );
}

function jobCondition(name) {
  const body = jobs.get(name);
  assert.ok(body, `missing job ${name}`);
  const folded = body.match(/^    if: >-\n((?: {6}.+\n)+)/m);
  return folded ? folded[1].split('\n').map((line) => line.trim()).join(' ').trim() : body.match(/^    if: (.+)$/m)?.[1];
}

const runs = {
  tag: { ref: 'refs/tags/v0.7.0', event: 'push' },
  main: { ref: 'refs/heads/main', event: 'push' },
  pull: { ref: 'refs/pull/9/merge', event: 'pull_request' },
  full: { ref: 'refs/heads/main', event: 'workflow_dispatch', mode: 'full' },
  smoke: { ref: 'refs/heads/main', event: 'workflow_dispatch', mode: 'smoke' },
  warm: { ref: 'refs/heads/main', event: 'workflow_dispatch', mode: 'warm' },
};

test('warm dispatch runs the release build jobs and nothing that signs or publishes', () => {
  assert.match(workflow, /^ {10}- warm$/m);
  const builds = ['release-credentials', 'web-assets', 'build-native-app', 'build-release'];
  const sideEffects = [...jobs.keys()].filter((name) =>
    name === 'sign-macos' || name === 'create-release' || name === 'attach-macos' || /^(publish|update)-/.test(name));
  assert.ok(sideEffects.length >= 8, `expected signing and publishing jobs, found ${sideEffects}`);
  for (const name of builds) {
    for (const run of ['tag', 'full', 'warm']) assert.equal(Boolean(evaluate(jobCondition(name), runs[run])), true, `${name} on ${run}`);
    for (const run of ['main', 'pull']) assert.equal(Boolean(evaluate(jobCondition(name), runs[run])), false, `${name} on ${run}`);
  }
  for (const name of sideEffects) assert.equal(Boolean(evaluate(jobCondition(name), runs.warm)), false, `${name} on warm`);
  assert.equal(Boolean(evaluate(jobCondition('sign-macos'), runs.full)), true);
  assert.equal(Boolean(evaluate(jobCondition('sign-macos'), runs.tag)), true);
  assert.equal(Boolean(evaluate(jobCondition('build-release-smoke'), runs.warm)), false);
});

test('warm refuses every ref but main before signing can depend on it', () => {
  const credentials = jobs.get('release-credentials');
  const refusal = credentials.slice(credentials.indexOf('- name: Refuse warm outside main')).split('\n      - ')[0];
  const guard = refusal.match(/^        if: (.+)$/m)[1];
  const warmOnTag = { ref: 'refs/tags/v0.7.0', event: 'workflow_dispatch', mode: 'warm' };
  assert.equal(evaluate(guard, warmOnTag), true);
  for (const run of ['warm', 'full', 'tag']) assert.equal(evaluate(guard, runs[run]), false, run);
  assert.match(refusal, /^ {10}exit 1$/m);
  assert.ok(credentials.indexOf('Refuse warm outside main') < credentials.indexOf('uses: actions/checkout'));
  // A failed credentials job skips every job that signs or builds a release.
  for (const name of ['sign-macos', 'build-native-app', 'build-release']) {
    assert.match(jobs.get(name), /^    needs: \[[^\]]*release-credentials[^\]]*\]$/m, name);
  }
});

test('warm and smoke skip the normal lanes; tags, full dispatch, and main keep them', () => {
  const outputs = Object.fromEntries([...jobs.get('changes').matchAll(/^ {6}(\w+): (\$\{\{.+\}\})$/gm)]
    .map(([, name, expression]) => [name, expression]));
  for (const name of ['rust', 'python', 'python_generated', 'sdk', 'ui', 'compat']) {
    for (const run of ['warm', 'smoke']) assert.equal(evaluate(outputs[name], runs[run]), 'false', `${name} on ${run}`);
    for (const run of ['tag', 'full']) assert.equal(evaluate(outputs[name], runs[run]), 'true', `${name} on ${run}`);
    assert.equal(evaluate(outputs[name], runs.main), 'filtered', `${name} on main`);
  }
  for (const run of ['warm', 'smoke', 'pull']) assert.equal(evaluate(outputs.full, runs[run]), false, `full on ${run}`);
  for (const run of ['tag', 'full', 'main']) assert.equal(evaluate(outputs.full, runs[run]), true, `full on ${run}`);
});

test('a warm run never shares a concurrency group with main pushes', () => {
  const group = workflow.match(/^concurrency:\n(?: {2}#.*\n)*  group: (.+)$/m)[1];
  const resolve = (run) => group.replace(/\$\{\{ (.+?) \}\}/g, (_, expression) => {
    const context = { workflow: 'CI/CD', ...runs[run] };
    if (expression === 'github.workflow') return context.workflow;
    if (expression === 'github.ref') return context.ref;
    return evaluate(expression, runs[run]);
  });
  assert.equal(resolve('main'), 'CI/CD-refs/heads/main');
  assert.equal(resolve('full'), 'CI/CD-refs/heads/main');
  assert.equal(resolve('warm'), 'CI/CD-refs/heads/main-warm');
});

test('release lanes write R2 without saving Actions archives, Tauri compiles included', () => {
  for (const name of ['web-assets', 'build-native-app', 'build-release']) {
    const cache = jobs.get(name).match(/uses: \.\/\.github\/actions\/rust-build-cache\n {8}with:\n((?: {10}.+\n)+)/)[1];
    assert.match(cache, /^ {10}save-if: "false"$/m, name);
    const uploads = [...jobs.get(name).matchAll(/uses: actions\/upload-artifact@v7\n {8}with:\n((?: {10}.+\n)+)/g)];
    assert.ok(uploads.length > 0, name);
    for (const [, inputs] of uploads) {
      assert.match(inputs, /^ {10}retention-days: \$\{\{ inputs\.release_artifacts == 'warm' && 1 \|\| (0|7) \}\}$/m, name);
    }
  }
  const native = jobs.get('build-native-app');
  for (const step of ['Build Tauri native bundle', 'Build unsigned macOS app bundle']) {
    const body = native.slice(native.indexOf(`- name: ${step}`)).split('\n      - name:')[0];
    assert.match(body, /^ {10}RUSTC_WRAPPER: sccache$/m, step);
  }
});

test('the nightly scheduler dispatches warm on main and nothing else', () => {
  assert.match(scheduler, /^ {2}schedule:\n {4}- cron: "[^"]+"$/m);
  assert.match(scheduler, /^ {2}workflow_dispatch:$/m);
  assert.match(scheduler, /^ {2}actions: write$/m);
  assert.match(scheduler, /^ {4}if: github\.repository == 'hyperb1iss\/hypercolor'$/m);
  assert.doesNotMatch(scheduler, /contents: write|secrets\./);
  const command = scheduler.match(/run: >-\n((?: {10}.+\n)+)/)[1].replace(/\s+/g, ' ').trim();
  assert.equal(command, 'gh workflow run ci.yml --repo "${GITHUB_REPOSITORY}" --ref main -f release_artifacts=warm');
});
