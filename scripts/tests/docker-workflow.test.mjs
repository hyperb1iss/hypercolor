import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');

function job(name) {
  const start = workflow.indexOf(`\n  ${name}:\n`);
  assert.ok(start >= 0, `missing workflow job ${name}`);
  const remainder = workflow.slice(start + 1);
  const next = remainder.slice(1).search(/^  [\w-]+:\n/m);
  return next < 0 ? remainder : remainder.slice(0, next + 1);
}

function stepScript(jobName, name) {
  const step = job(jobName).split(`      - name: ${name}\n`)[1]?.split('\n      - ')[0];
  assert.ok(step, `missing ${jobName} step ${name}`);
  return step.split('        run: |\n')[1].split('\n')
    .filter((line) => line.startsWith('          ') || !line.trim())
    .map((line) => line.slice(10)).join('\n');
}

test('release image builds reuse verified native Linux distributions before export', () => {
  for (const [name, verification] of [
    ['build-release', 'Verify Linux release tarball'],
    ['build-release-smoke', 'Verify release tarball'],
  ]) {
    const body = job(name);
    const verified = body.indexOf(`- name: ${verification}`);
    const staged = body.indexOf('- name: Stage verified Docker runtime payload');
    const built = body.indexOf('- name: Build Docker runtime');
    const proven = body.indexOf('- name: Prove container rendering, WLED output, and persistence');
    const exported = body.indexOf('- name: Export tested Docker image');
    assert.ok(verified >= 0 && verified < staged && staged < built && built < proven && proven < exported);
    assert.match(body, /tar -xzf "dist\/\$\{\{ steps\.version\.outputs\.dist_name \}\}\.tar\.gz"/);
    assert.match(body, /context: docker-context\n\s+file: packaging\/docker\/Dockerfile\n\s+load: true/);
    assert.match(body, /run: node scripts\/tests\/docker-smoke\.mjs hypercolor-container:/);
    assert.doesNotMatch(body, /packages: write|docker\/login-action|push: true/);
  }
  assert.match(job('build-release'), /target: linux-amd64\n\s+os: ubuntu-latest/);
  assert.match(job('build-release'), /target: linux-arm64\n\s+os: ubuntu-24\.04-arm/);
});

test('PR and main Docker smoke reuses the Servo e2e stack without rebuilding Rust', () => {
  const body = job('docker-e2e');
  assert.match(body, /needs: e2e-assemble/);
  assert.match(body, /name: e2e-stack-servo/);
  assert.match(body, /github\.event_name == 'pull_request'/);
  assert.match(body, /github\.ref == 'refs\/heads\/main'/);
  assert.match(body, /run: node scripts\/tests\/docker-smoke\.mjs hypercolor-container:proof/);
  assert.doesNotMatch(body, /cargo (?:build|test)|packages: write|push: true/);
  const filters = job('changes');
  for (const domain of ['rust', 'ui', 'workflow']) {
    const body = filters.split(`            ${domain}:\n`)[1]?.split(/\n            \w+:\n/)[0];
    assert.ok(body, `missing ${domain} change filter`);
    assert.match(body, /'packaging\/(?:\*\*|docker\/\*\*)'/);
    assert.match(body, /'scripts\/tests\/docker-smoke\.mjs'/);
    assert.match(body, /'scripts\/tests\/docker-workflow\.test\.mjs'/);
  }
});

test('registry publication waits for full release gates and never rebuilds images', () => {
  const body = job('publish-docker');
  assert.match(body, /needs: \[create-release, build-release\]/);
  assert.match(job('create-release'), /needs: \[[^\n]*build-release[^\n]*rust-test-servo[^\n]*e2e/);
  assert.match(body, /startsWith\(github\.ref, 'refs\/tags\/'\)/);
  assert.match(body, /github\.repository == 'hyperb1iss\/hypercolor'/);
  assert.match(body, /packages: write/);
  assert.match(body, /pattern: container-linux-\*/);
  assert.match(body, /docker image load --input/);
  assert.match(body, /docker buildx imagetools create/);
  assert.doesNotMatch(body, /docker\/build-push-action|cargo build|contents: write/);
  assert.equal((workflow.match(/packages: write/g) ?? []).length, 1);
});

test('cross-tag publication protects latest without dropping queued releases', () => {
  const body = job('publish-docker');
  assert.match(body, /concurrency:\n\s+group: hypercolor-docker-publication\n\s+cancel-in-progress: false\n\s+queue: max/);
  assert.equal((workflow.match(/^\s+queue:/gm) ?? []).length, 1);
  for (const name of ['docker-e2e', 'build-release', 'build-release-smoke']) {
    assert.doesNotMatch(job(name), /hypercolor-docker-publication/);
  }
});

function publishFixture(version, latestTag, wrongArchitecture = false) {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-docker-publication-'));
  const log = path.join(root, 'commands');
  try {
    writeFileSync(path.join(root, 'docker'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$DOCKER_TEST_LOG"
case "$*" in
  'image load --input container-images/hypercolor-container-linux-'*.tar) ;;
  'image inspect hypercolor-container:linux-amd64 --format {{.Architecture}}') printf '%s\\n' amd64 ;;
  'image inspect hypercolor-container:linux-arm64 --format {{.Architecture}}')
    if [[ "$DOCKER_TEST_WRONG_ARCH" == 1 ]]; then printf '%s\\n' amd64; else printf '%s\\n' arm64; fi ;;
  'image tag hypercolor-container:linux-'*) ;;
  'image push ghcr.io/hyperb1iss/hypercolor:'*) ;;
  'image inspect ghcr.io/hyperb1iss/hypercolor:'*' --format {{index .RepoDigests 0}}')
    if [[ "$*" == *-amd64* ]]; then
      printf '%s\\n' ghcr.io/hyperb1iss/hypercolor@sha256:aaaaaaaa
    else
      printf '%s\\n' ghcr.io/hyperb1iss/hypercolor@sha256:bbbbbbbb
    fi ;;
  'buildx imagetools create '*|'buildx imagetools inspect '*) ;;
  *) printf 'unexpected docker command: %s\\n' "$*" >&2; exit 99 ;;
esac
`, { mode: 0o755 });
    writeFileSync(path.join(root, 'gh'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "gh $*" >> "$DOCKER_TEST_LOG"
[[ "$*" == 'api repos/hyperb1iss/hypercolor/releases/latest --jq .tag_name' ]] || exit 99
printf '%s\\n' "$DOCKER_TEST_LATEST_TAG"
`, { mode: 0o755 });
    const result = spawnSync('bash', ['-c', stepScript('publish-docker', 'Publish tested images and multiarch index')], {
      cwd: root,
      encoding: 'utf8',
      env: {
        ...process.env,
        PATH: `${root}:${process.env.PATH}`,
        GITHUB_REF_NAME: `v${version}`,
        GITHUB_REPOSITORY: 'hyperb1iss/hypercolor',
        DOCKER_TEST_LOG: log,
        DOCKER_TEST_LATEST_TAG: latestTag,
        DOCKER_TEST_WRONG_ARCH: wrongArchitecture ? '1' : '0',
      },
    });
    return { ...result, commands: readFileSync(log, 'utf8') };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test('stable images publish both tested digests with version and latest tags', () => {
  const result = publishFixture('0.5.2', 'v0.5.2');
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.commands, /image push ghcr\.io\/hyperb1iss\/hypercolor:0\.5\.2-amd64/);
  assert.match(result.commands, /image push ghcr\.io\/hyperb1iss\/hypercolor:0\.5\.2-arm64/);
  assert.match(result.commands, /imagetools create --tag ghcr\.io\/hyperb1iss\/hypercolor:0\.5\.2 --tag ghcr\.io\/hyperb1iss\/hypercolor:latest ghcr\.io\/hyperb1iss\/hypercolor@sha256:aaaaaaaa ghcr\.io\/hyperb1iss\/hypercolor@sha256:bbbbbbbb/);
});

test('prereleases and older stable reruns leave latest untouched', () => {
  for (const [version, latestTag] of [['0.5.3-rc.1', 'v0.5.2'], ['0.5.2', 'v0.5.3']]) {
    const result = publishFixture(version, latestTag);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.commands, /imagetools create --tag ghcr\.io\/hyperb1iss\/hypercolor:/);
    assert.doesNotMatch(result.commands, /--tag ghcr\.io\/hyperb1iss\/hypercolor:latest/);
    if (version.includes('-')) assert.doesNotMatch(result.commands, /gh api/);
  }
});

test('a mismatched archive architecture cannot publish a multiarch index', () => {
  const result = publishFixture('0.5.2', 'v0.5.2', true);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Docker image architecture amd64 does not match arm64/);
  assert.doesNotMatch(result.commands, /imagetools create|image push .*:0\.5\.2-arm64/);
});
