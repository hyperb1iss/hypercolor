import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
const job = workflow.match(/^  rust-check-macos:\n([\s\S]*?)(?=^  [a-z][\w-]*:)/m)?.[1];
assert.ok(job, 'the macOS release prerequisite must exist');
const steps = new Map([...job.matchAll(/^      - name: (.+)\n([\s\S]*?)(?=^      - |$(?![\s\S]))/gm)]
  .map(([, name, body]) => [name, body]));

function condition(name) {
  assert.ok(steps.has(name), `missing macOS check: ${name}`);
  return steps.get(name).match(/^        if: (.+)$/m)?.[1];
}

test('each macOS architecture has independent workspace and capability capacity', () => {
  const entries = [...job.matchAll(/          - label: (.+)\n            os: (.+)\n            expected-arch: (.+)\n            lane: (.+)/g)]
    .map(([, label, os, arch, lane]) => ({ label, os, arch, lane }));
  assert.equal(entries.length, 4);
  for (const [label, os, arch] of [['Apple Silicon', 'macos-26', 'arm64'], ['Intel', 'macos-26-intel', 'x86_64']]) {
    assert.deepEqual(entries.filter((entry) => entry.arch === arch),
      ['workspace', 'capabilities'].map((lane) => ({ label, os, arch, lane })));
  }
  assert.match(job, /^    needs: changes$/m);
  assert.match(job, /^      fail-fast: false$/m);
  assert.match(job, /^    timeout-minutes: 120$/m);
});

test('workspace artifacts and capability fixtures run in separate lanes', () => {
  for (const name of [
    'Seed native app frontend fixture', 'Check macOS workspace',
    'Build deployment and Sequoia availability fixtures', 'Verify deployment target',
    'Reject unguarded Tahoe symbols in the Sequoia artifact',
  ]) assert.equal(condition(name), "matrix.lane == 'workspace'");
  for (const name of [
    'Install nextest', 'Clippy macOS interop', 'Clippy macOS capture fixtures',
    'Clippy macOS host input and ownership', 'Run macOS interop fixtures',
    'Run macOS capture fixtures', 'Run macOS host input and ownership fixtures',
    'Run macOS status API fixtures',
  ]) assert.equal(condition(name), "matrix.lane == 'capabilities'");
  assert.equal(condition('Qualify Intel Metal fixture'),
    "matrix.lane == 'capabilities' && matrix.expected-arch == 'x86_64'");
  assert.equal(condition('Qualify macOS runner and SDK'), undefined);
  assert.equal(condition('Verify macOS signing secret transport'), undefined);
  assert.equal(condition('Install NASM'), "matrix.expected-arch == 'x86_64'");
});

test('cache ownership and restored target directories agree across lanes', () => {
  const target = 'rust-check-macos-${{ matrix.lane }}';
  assert.ok(job.includes(`CARGO_TARGET_DIR: \${{ github.workspace }}/.cache/hypercolor/target/${target}`));
  assert.ok(job.includes(`workspaces: . -> .cache/hypercolor/target/${target}`));
  assert.ok(job.includes('shared-key: rust-check-macos-${{ matrix.expected-arch }}-${{ matrix.lane }}'));
  assert.match(job, /cache-on-failure: "false"/);
  assert.equal(condition('Save Rust build caches'), 'always()');
});

test('publication retains every validation gate while compilation overlaps it', () => {
  const body = id => workflow.match(new RegExp(`^  ${id}:\\n([\\s\\S]*?)(?=^  [a-z][\\w-]*:)`, 'm'))?.[1];
  const needs = id => body(id)?.match(/^    needs: \[(.+)\]$/m)?.[1].split(', ');
  for (const id of ['build-release', 'build-native-app']) {
    assert.deepEqual(needs(id), ['release-credentials', 'web-assets']);
  }
  const validation = [
    'release-credentials', 'rust-check-shared', 'rust-check-macos', 'rust-test',
    'rust-test-servo', 'rust-windows', 'rust-deny', 'sdk', 'ui', 'e2e',
    'web-assets', 'python', 'python-generated',
  ];
  assert.deepEqual(needs('create-release'), ['build-release', 'build-native-app', ...validation]);
  // Preserve GitHub's implicit success() gate: failed or skipped validation
  // must never become publishable through always() or !cancelled().
  const condition = body('create-release').split('    needs:')[0];
  assert.doesNotMatch(condition, /always\(|cancelled\(|failure\(/);
  assert.match(condition, /startsWith\(github.ref, 'refs\/tags\/'\)/);
});
