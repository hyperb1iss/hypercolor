import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const action = readFileSync(new URL('./action.yml', import.meta.url), 'utf8');
const steps = new Map(action.split('\n    - name: ').slice(1).map((step) => {
  const end = step.indexOf('\n');
  return [step.slice(0, end), step.slice(end)];
}));

function enabled(name, env, status = 'success') {
  const condition = steps.get(name).match(/\n      if: >-\n((?:        .+\n)+)/)[1].trim();
  return Function('env', 'job', 'inputs', 'always', `return ${condition}`)(
    env, { status }, { phase: 'save' }, () => true,
  );
}

test('restore outcomes cross invocations and only exact hits suppress immutable saves', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-cache-restore-'));
  try {
    for (const registry of ['true', 'false', '']) {
      for (const build of ['true', 'false', '']) {
        const environment = path.join(root, `env-${registry}-${build}`);
        execFileSync(process.execPath, [fileURLToPath(new URL('./restore-state.mjs', import.meta.url))], {
          env: { ...process.env, GITHUB_ENV: environment,
            CACHE_REGISTRY_EXACT_HIT: registry, CACHE_BUILD_EXACT_HIT: build },
        });
        const restored = Object.fromEntries(readFileSync(environment, 'utf8').trim().split('\n')
          .map((line) => line.split('=')));
        const env = { ...restored, HYPERCOLOR_CACHE_WRITE: 'true', HYPERCOLOR_CACHE_SAVE_FAILURE: 'true' };
        assert.equal(restored.HYPERCOLOR_REGISTRY_CACHE_EXACT_HIT, String(registry === 'true'));
        assert.equal(restored.HYPERCOLOR_BUILD_CACHE_EXACT_HIT, String(build === 'true'));
        for (const status of ['success', 'failure']) {
          assert.equal(enabled('Save Cargo downloads', env, status), registry !== 'true');
          assert.equal(enabled('Capture source timestamps with content fingerprints', env, status), build !== 'true');
          assert.equal(enabled('Save build artifacts and compiler cache', env, status), build !== 'true');
        }
        for (const name of ['Save Cargo downloads', 'Capture source timestamps with content fingerprints',
          'Save build artifacts and compiler cache']) {
          assert.equal(enabled(name, { ...env, HYPERCOLOR_CACHE_WRITE: 'false' }), false);
          assert.equal(enabled(name, { ...env, HYPERCOLOR_CACHE_SAVE_FAILURE: 'false' }, 'failure'), false);
        }
      }
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('composite restore outputs are persisted even if a later restore fails', () => {
  assert.match(steps.get('Restore Cargo downloads'), /\n      id: registry-restore\n/);
  assert.match(steps.get('Restore build artifacts and compiler cache'), /\n      id: build-restore\n/);
  const persist = steps.get('Persist cache restore outcomes');
  assert.match(persist, /if: always\(\) && inputs.phase == 'restore'/);
  assert.match(persist, /CACHE_REGISTRY_EXACT_HIT: \$\{\{ steps.registry-restore.outputs.cache-hit \}\}/);
  assert.match(persist, /CACHE_BUILD_EXACT_HIT: \$\{\{ steps.build-restore.outputs.cache-hit \}\}/);
  assert.match(persist, /run: node "\$CACHE_ACTION_PATH\/restore-state.mjs"/);
});
