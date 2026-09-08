import { existsSync, readFileSync, unlinkSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export function refreshNativeWrappers(env = process.env) {
  const cacheRoot = env.HYPERCOLOR_CACHE_DIR || path.join(env.GITHUB_WORKSPACE, '.cache/hypercolor');
  let removed = 0;
  for (const [name, compiler] of [['cc', 'cc'], ['cxx', 'c++']]) {
    const wrapper = path.join(cacheRoot, 'toolchain', name);
    if (!existsSync(wrapper)) continue;
    const source = readFileSync(wrapper, 'utf8');
    const prefix = source.match(/^#!\/usr\/bin\/env bash\nexec "([^"\n]+\/sccache)" /);
    if (!prefix || source !== `${prefix[0]}"$(command -v ${compiler})" "$@"\n`) continue;
    // cc-rs already adds RUSTC_WRAPPER=sccache. Regenerate only the engine's
    // old sccache-backed scripts so they use the installed ccache backend.
    unlinkSync(wrapper);
    removed += 1;
  }
  return removed;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(`Retired ${refreshNativeWrappers()} nested sccache compiler wrappers`);
}
