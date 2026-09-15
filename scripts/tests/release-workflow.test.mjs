import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const workflow = readFileSync(new URL('../../.github/workflows/release.yml', import.meta.url), 'utf8');

function stepScript(name) {
  const step = workflow.split(`      - name: ${name}\n`)[1]?.split('\n      - ')[0];
  assert.ok(step, `missing workflow step ${name}`);
  return step.split('        run: |\n')[1].split('\n')
    .filter((line) => line.startsWith('          ') || !line.trim())
    .map((line) => line.slice(10)).join('\n');
}

function validateVersion(tags, version) {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-release-workflow-'));
  try {
    for (const args of [
      ['init', '-q'],
      ['-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '--allow-empty', '-qm', 'fixture'],
      ...tags.map((tag) => ['tag', tag]),
    ]) {
      const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
      assert.equal(result.status, 0, result.stderr);
    }
    // Registry fixtures model an unpublished version, without network access.
    writeFileSync(path.join(root, 'curl'), '#!/bin/sh\nexit 22\n', { mode: 0o755 });
    return spawnSync('bash', ['-c', stepScript('Validate release version')], {
      cwd: root,
      encoding: 'utf8',
      env: { ...process.env, PATH: `${root}:${process.env.PATH}`, INPUT_VERSION: version, GITHUB_OUTPUT: path.join(root, 'outputs') },
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test('stable promotion is newer than its release candidate', () => {
  const result = validateVersion(['v0.5.1', 'v0.5.2-rc.1'], '0.5.2');
  assert.equal(result.status, 0, result.stderr);
});

test('prereleases cannot follow their already-published stable version', () => {
  const result = validateVersion(['v0.5.2-rc.1', 'v0.5.2'], '0.5.2-rc.2');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /not above the latest tag v0.5.2\n/);
});

test('prerelease sequencing remains numeric and rejects downgrades', () => {
  for (const [previous, next, passes] of [
    ['0.5.2-rc.2', '0.5.2-rc.10', true],
    ['0.5.2-beta.2', '0.5.2-rc.1', true],
    ['0.5.2-rc.10', '0.5.2-rc.2', false],
    ['0.5.2-rc.1', '0.5.2-beta.3', false],
    ['0.5.2', '0.5.1', false],
  ]) {
    const result = validateVersion([`v${previous}`], next);
    assert.equal(result.status === 0, passes, `${previous} -> ${next}: ${result.stderr}`);
  }
});

test('source CI accepts main pushes and exact release tags, excluding branch artifact runs', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-release-source-'));
  try {
    writeFileSync(path.join(root, 'git'), `#!/bin/sh
case "$*" in
  'rev-parse HEAD') printf '%s\\n' source-commit ;;
  *'refs/tags/v0.5.2^{commit}'*) printf '%s\\n' "$RELEASE_TEST_TAG_COMMIT" ;;
  *) exit 1 ;;
esac
`, { mode: 0o755 });
    writeFileSync(path.join(root, 'gh'), `#!/bin/sh
case "$*" in
  'run list --workflow ci.yml --commit source-commit '*) printf '%s\\n' "$RELEASE_TEST_RUNS" ;;
  'run watch 42 --exit-status') exit "$RELEASE_TEST_WATCH_STATUS" ;;
  *) echo "Unexpected gh invocation: $*" >&2; exit 99 ;;
esac
`, { mode: 0o755 });
    for (const [ref, event, tagCommit, watchStatus, passes] of [
      ['main', 'push', '', '0', true],
      ['v0.5.2', 'workflow_dispatch', 'source-commit', '0', true],
      ['v0.5.2', 'push', 'source-commit', '0', true],
      ['main', 'workflow_dispatch', '', '0', false],
      ['feature', 'workflow_dispatch', '', '0', false],
      ['v0.5.2', 'workflow_dispatch', 'different-commit', '0', false],
      ['v0.5.2', 'pull_request', 'source-commit', '0', false],
      ['v0.5.2', 'workflow_dispatch', 'source-commit', '1', false],
      ['main', 'push', '', '1', false],
    ]) {
      const result = spawnSync('bash', ['-c', stepScript('Require passing CI for the release source')], {
        encoding: 'utf8',
        env: {
          ...process.env, PATH: `${root}:${process.env.PATH}`,
          RELEASE_TEST_RUNS: `42\t${ref}\t${event}`, RELEASE_TEST_TAG_COMMIT: tagCommit,
          RELEASE_TEST_WATCH_STATUS: watchStatus,
        },
      });
      assert.equal(result.status === 0, passes, `${ref}/${event}/${tagCommit}/${watchStatus}: ${result.stderr}`);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('signing report accepts complete credentials and lists only the missing names', () => {
  const names = [
    'APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_SIGNING_IDENTITY',
    'APPLE_TEAM_ID', 'APPLE_API_KEY_ID', 'APPLE_API_ISSUER', 'APPLE_API_KEY_CONTENT',
  ];
  const env = { ...process.env, ...Object.fromEntries(names.map(name => [name, 'fixture'])) };
  const script = stepScript('Report macOS signing credentials');
  const complete = spawnSync('bash', ['-c', script], { encoding: 'utf8', env });
  assert.equal(complete.status, 0, complete.stderr);
  assert.match(complete.stdout, /credentials are configured/);
  assert.equal(complete.stderr, '');
  // A missing secret narrows the release to Linux and Windows; it never blocks the cut.
  const missing = spawnSync('bash', ['-c', script], {
    encoding: 'utf8', env: { ...env, APPLE_CERTIFICATE: '', APPLE_API_ISSUER: '' },
  });
  assert.equal(missing.status, 0, missing.stderr);
  assert.match(missing.stderr, /Linux and Windows only\. Missing: APPLE_CERTIFICATE APPLE_API_ISSUER$/m);
  assert.match(missing.stdout, /^::warning::.*missing APPLE_CERTIFICATE APPLE_API_ISSUER/m);
  assert.doesNotMatch(missing.stderr, /APPLE_TEAM_ID/);
  assert.doesNotMatch(readFileSync(new URL('../../.github/workflows/release.yml', import.meta.url), 'utf8'),
    /name: Report macOS signing credentials\n        if:/, 'the report runs on dry runs too');
});

test('dispatch resumes the prepared tag and refuses a moved tag', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-release-dispatch-'));
  try {
    writeFileSync(path.join(root, 'git'), '#!/bin/sh\nprintf "%s\\n" "$RELEASE_TEST_CHECKOUT"\n', { mode: 0o755 });
    writeFileSync(path.join(root, 'gh'), '#!/bin/sh\nprintf "%s\\n" "$*" > "$RELEASE_TEST_DISPATCH"\n', { mode: 0o755 });
    const options = {
      encoding: 'utf8',
      env: {
        ...process.env, PATH: `${root}:${process.env.PATH}`, TAG: 'v0.5.2',
        RELEASE_COMMIT: 'prepared-commit', RELEASE_TEST_DISPATCH: path.join(root, 'dispatched'),
        RELEASE_TEST_CHECKOUT: 'moved-commit', GITHUB_STEP_SUMMARY: path.join(root, 'summary'),
      },
    };
    const result = spawnSync('bash', ['-c', stepScript('Dispatch the prepared release')], options);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /no longer points to prepared commit/);
    assert.throws(() => readFileSync(path.join(root, 'dispatched')), { code: 'ENOENT' });
    options.env.RELEASE_TEST_CHECKOUT = 'prepared-commit';
    const resumed = spawnSync('bash', ['-c', stepScript('Dispatch the prepared release')], options);
    assert.equal(resumed.status, 0, resumed.stderr);
    assert.equal(readFileSync(path.join(root, 'dispatched'), 'utf8').trim(), 'workflow run ci.yml --ref v0.5.2');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
