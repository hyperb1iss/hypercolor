import { createHash, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { appendFileSync, existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const digest = (value) => createHash('sha256').update(JSON.stringify(value)).digest('hex').slice(0, 20);
const lines = (value = '') => value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);

// Command-list variables and checkout locations are not compiler inputs.
// Cargo's fingerprints still validate artifacts restored after source edits.
export function compilerEnvironment(env) {
  const exact = new Set([
    'RUSTFLAGS', 'RUSTDOCFLAGS', 'RUSTC_BOOTSTRAP', 'CARGO_INCREMENTAL',
    'CARGO_ENCODED_RUSTFLAGS', 'CARGO_ENCODED_RUSTDOCFLAGS',
    'CC', 'CXX', 'AR', 'ARFLAGS', 'LDFLAGS', 'CL', '_CL_',
  ]);
  const apple = new Set(['MACOSX_DEPLOYMENT_TARGET', 'SDKROOT', 'DEVELOPER_DIR', 'XCODE_VERSION']);
  return Object.entries(env).filter(([key, value]) => value && key !== 'CARGO_TARGET_DIR' && (
    exact.has(key) || /^(CARGO_(PROFILE_|BUILD_|TARGET_)|CC_|CXX_|CFLAGS|CXXFLAGS|CMAKE_)/.test(key) ||
    (env.RUNNER_OS === 'macOS' && apple.has(key))
  )).sort(([a], [b]) => a.localeCompare(b));
}

export function cacheWriter(saveIf, ref, defaultBranch, eventName) {
  if (!['auto', 'true', 'false'].includes(saveIf)) throw new Error(`Invalid save-if: ${saveIf}`);
  // A caller cannot turn an untrusted pull request or tag into a writer.
  return saveIf !== 'false' && Boolean(defaultBranch) &&
    ref === `refs/heads/${defaultBranch}` && ['push', 'workflow_dispatch', 'schedule'].includes(eventName);
}

// The shared compiler cache lives in Cloudflare R2 when the repository
// provides credentials. Only the trusted cache writer adds entries; every
// other run reads, and a run without credentials (a fork pull request) keeps
// the local disk cache.
export function remoteCompilerCache(env, write) {
  const settings = {
    keyId: env.SCCACHE_R2_ACCESS_KEY_ID, secret: env.SCCACHE_R2_SECRET_ACCESS_KEY,
    bucket: env.SCCACHE_R2_BUCKET, endpoint: env.SCCACHE_R2_ENDPOINT,
  };
  if (!settings.keyId || !settings.secret) return { reason: 'no R2 credentials in this run' };
  if (!settings.bucket || !settings.endpoint) return { reason: 'R2 bucket or endpoint is not configured' };
  if (!/^https:\/\/[^/]+\/?$/.test(settings.endpoint)) return { reason: 'R2 endpoint must be an https origin' };
  return {
    settings,
    values: {
      SCCACHE_BUCKET: settings.bucket,
      SCCACHE_ENDPOINT: settings.endpoint.replace(/\/$/, ''),
      SCCACHE_REGION: 'auto',
      SCCACHE_S3_KEY_PREFIX: `sccache/${String(env.RUNNER_OS).toLowerCase()}-${String(env.RUNNER_ARCH).toLowerCase()}`,
      SCCACHE_S3_RW_MODE: write ? 'READ_WRITE' : 'READ_ONLY',
      AWS_ACCESS_KEY_ID: settings.keyId,
      AWS_SECRET_ACCESS_KEY: settings.secret,
    },
  };
}

// sccache refuses to start when its remote storage check fails, which would
// fail the job over an optional accelerator. A signed HEAD request proves the
// endpoint answers and the credentials are accepted before R2 is enabled;
// 404 is the normal answer for a missing object.
export function probeRemoteCache({ endpoint, bucket, keyId, secret }, run = execFileSync) {
  const url = `${endpoint.replace(/\/$/, '')}/${bucket}/.hypercolor-cache-probe`;
  try {
    const status = run('curl', [
      '--silent', '--show-error', '--head', '--max-time', '15',
      '--output', process.platform === 'win32' ? 'NUL' : '/dev/null',
      '--write-out', '%{http_code}', '--aws-sigv4', 'aws:amz:auto:s3', '--config', '-', url,
    ], { input: `user = "${keyId}:${secret}"\n`, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] });
    const code = String(status).trim().slice(-3);
    return { ok: code === '200' || code === '404', detail: `HTTP ${code}` };
  } catch (error) {
    const code = String(error.stdout ?? '').trim().slice(-3);
    const message = String(error.stderr ?? error.message ?? '').trim().split('\n')[0];
    return { ok: false, detail: code && code !== '000' ? `HTTP ${code}` : message || 'curl failed' };
  }
}

export function cacheKeys({ shape, variant, runner, compiler, environment, workspaces, revisions, locks }) {
  const owner = [shape, variant].filter(Boolean).join('-');
  if (!/^[a-zA-Z0-9_.-]+$/.test(owner)) throw new Error(`Invalid cache owner: ${owner}`);
  const prefix = `hypercolor-build-v3-${owner}-${runner}-${digest([compiler, environment, workspaces])}-`;
  const registryPrefix = `hypercolor-registry-v3-${runner}-`;
  return {
    prefix,
    key: `${prefix}${digest(locks)}-${digest(revisions)}`,
    registryPrefix,
    registryKey: `${registryPrefix}${digest(locks)}`,
  };
}

function exportEnvironment(values, destination) {
  for (const [key, value] of Object.entries(values)) {
    const delimiter = `cache_${randomUUID()}`;
    appendFileSync(destination, `${key}<<${delimiter}\n${value}\n${delimiter}\n`);
  }
}

export function configure(env = process.env, compilerVersion, probe = probeRemoteCache) {
  const workspace = env.GITHUB_WORKSPACE;
  if (!workspace || !env.GITHUB_ENV) throw new Error('GitHub workspace and environment file are required');
  const mappings = lines(env.CACHE_WORKSPACES).map((line) => {
    const parts = line.split('->').map((part) => part.trim());
    if (parts.length !== 2 || parts.some((part) => !part)) throw new Error(`Invalid workspace mapping: ${line}`);
    return parts;
  });
  if (!mappings.length) throw new Error('At least one workspace mapping is required');
  const cacheRoot = env.HYPERCOLOR_CACHE_DIR || path.join(workspace, '.cache/hypercolor');
  const compilerCache = path.join(cacheRoot, 'sccache');
  const nativeCache = env.RUNNER_OS === 'macOS'
    ? path.resolve(workspace, env.CCACHE_DIR || path.join(cacheRoot, 'ccache')) : undefined;
  const sourceTimes = path.join(cacheRoot, 'source-mtimes.json');
  const profile = {
    CARGO_INCREMENTAL: '0',
    CARGO_PROFILE_DEV_DEBUG: '0',
    CARGO_PROFILE_TEST_DEBUG: '0',
    CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG: '0',
    CARGO_PROFILE_TEST_BUILD_OVERRIDE_DEBUG: '0',
  };
  const revisions = [env.GITHUB_SHA];
  const locks = [];
  for (const [root] of mappings) {
    const cwd = path.resolve(workspace, root);
    revisions.push(execFileSync('git', ['rev-parse', 'HEAD'], { cwd, encoding: 'utf8' }).trim());
    const files = execFileSync('git', ['ls-files', '-z', 'Cargo.lock', '**/Cargo.lock'], { cwd, encoding: 'utf8' });
    for (const file of files.split('\0').filter(Boolean).sort()) {
      const absolute = path.join(cwd, file);
      if (existsSync(absolute)) locks.push([root, file, readFileSync(absolute, 'utf8')]);
    }
  }
  const keys = cacheKeys({
    shape: env.CACHE_SHAPE || 'workspace', variant: env.CACHE_VARIANT,
    runner: `${env.RUNNER_OS}-${env.RUNNER_ARCH}`,
    compiler: compilerVersion ?? execFileSync('rustc', ['-vV'], { encoding: 'utf8' }),
    environment: compilerEnvironment({ ...env, ...profile }),
    workspaces: mappings, revisions, locks,
  });
  const cargoHome = env.CARGO_HOME || path.join(homedir(), '.cargo');
  const write = cacheWriter(env.CACHE_SAVE_IF || 'auto', env.GITHUB_REF, env.CACHE_DEFAULT_BRANCH, env.GITHUB_EVENT_NAME);
  const remote = remoteCompilerCache(env, write);
  let remoteValues = {};
  let compilerCacheLocation = `local disk (${remote.reason})`;
  if (remote.values) {
    for (const secret of [remote.settings.keyId, remote.settings.secret]) console.log(`::add-mask::${secret}`);
    const reachable = probe(remote.settings);
    if (reachable.ok) {
      remoteValues = remote.values;
      compilerCacheLocation = `R2 ${remote.values.SCCACHE_BUCKET} ${remote.values.SCCACHE_S3_RW_MODE.toLowerCase().replace('_', '-')}`;
    } else {
      compilerCacheLocation = `local disk (R2 unavailable: ${reachable.detail})`;
      console.log(`::warning::Shared compiler cache unavailable (${reachable.detail}); this job compiles with the local disk cache.`);
    }
  }
  // actions/cache only restores entries whose version, a hash of this path
  // list, matches. The list must not depend on whether R2 holds the compiler
  // cache, or R2 jobs, local-disk jobs, and every existing entry would stop
  // restoring each other's target directories.
  const buildPaths = new Set([
    ...mappings.map(([root, target]) => path.resolve(workspace, root, target)), compilerCache, sourceTimes,
    ...(nativeCache ? [nativeCache] : []),
    ...lines(env.CACHE_DIRECTORIES).map((directory) => path.resolve(workspace, directory)),
  ]);
  const values = {
    ...profile,
    CCACHE_COMPRESS: 'true', CCACHE_MAXSIZE: '500M',
    ...(nativeCache ? { CCACHE_DIR: nativeCache } : {}),
    SCCACHE_DIR: compilerCache, SCCACHE_CACHE_SIZE: '3G',
    ...(env.RUNNER_OS === 'Windows' ? {} : { SCCACHE_SERVER_UDS: path.join(cacheRoot, 'sccache.sock') }),
    HYPERCOLOR_CACHE_SOURCE_ROOTS: JSON.stringify(mappings.map(([root]) => path.resolve(workspace, root))),
    HYPERCOLOR_CACHE_SOURCE_TIMES: sourceTimes,
    HYPERCOLOR_BUILD_CACHE_KEY: keys.key,
    HYPERCOLOR_BUILD_CACHE_PREFIX: keys.prefix,
    HYPERCOLOR_BUILD_CACHE_PATHS: [...buildPaths].join('\n'),
    HYPERCOLOR_REGISTRY_CACHE_KEY: keys.registryKey,
    HYPERCOLOR_REGISTRY_CACHE_PREFIX: keys.registryPrefix,
    HYPERCOLOR_REGISTRY_CACHE_PATHS: ['registry', 'git'].map((directory) => path.join(cargoHome, directory)).join('\n'),
    HYPERCOLOR_CACHE_WRITE: String(write),
    HYPERCOLOR_CACHE_SAVE_FAILURE: env.CACHE_ON_FAILURE_INPUT === 'true' ? 'true' : 'false',
    HYPERCOLOR_REGISTRY_CACHE_EXACT_HIT: 'false',
    HYPERCOLOR_BUILD_CACHE_EXACT_HIT: 'false',
    HYPERCOLOR_COMPILER_CACHE: compilerCacheLocation,
    ...remoteValues,
  };
  exportEnvironment(values, env.GITHUB_ENV);
  console.log(`Build cache: ${keys.key}\nCompatible restore prefix: ${keys.prefix}\nCache writer: ${values.HYPERCOLOR_CACHE_WRITE}\nCompiler cache: ${compilerCacheLocation}`);
  return values;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) configure();
