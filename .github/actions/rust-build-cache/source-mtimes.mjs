import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import {
  existsSync, lstatSync, mkdirSync, readFileSync, readlinkSync, realpathSync, statSync, utimesSync, writeFileSync,
} from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';

function sourceFiles(root) {
  return execFileSync('git', ['ls-files', '-z', '--cached'], { cwd: root, encoding: 'utf8' })
    .split('\0').filter(Boolean);
}

function regularSource(root, relative) {
  if (path.isAbsolute(relative)) return undefined;
  const absolute = path.resolve(root, relative);
  const inside = path.relative(root, absolute);
  if (inside.startsWith(`..${path.sep}`) || inside === '..') return undefined;
  if (!existsSync(absolute) || !lstatSync(absolute).isFile()) return undefined;
  const resolved = path.relative(realpathSync(root), realpathSync(absolute));
  if (resolved.startsWith(`..${path.sep}`) || resolved === '..') return undefined;
  return absolute;
}

const contentHash = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');

function sourceSymlinks(root, files) {
  return files.flatMap((relative) => {
    const absolute = path.resolve(root, relative);
    if (!lstatSync(absolute, { throwIfNoEntry: false })?.isSymbolicLink()) return [];
    return [{ path: relative, target: readlinkSync(absolute) }];
  });
}

export function captureSourceTimes(roots, destination) {
  const tracked = roots.map(sourceFiles);
  const symlinks = roots.map((root, index) => sourceSymlinks(root, tracked[index]));
  const sources = roots.map((root, index) => tracked[index].flatMap((relative) => {
    const absolute = regularSource(root, relative);
    if (!absolute) return [];
    const stat = statSync(absolute);
    return [{ path: relative, hash: contentHash(absolute), executable: stat.mode & 0o111, mtimeMs: stat.mtimeMs }];
  }));
  mkdirSync(path.dirname(destination), { recursive: true });
  writeFileSync(destination, JSON.stringify({ version: 2, sources, symlinks }));
  return sources.reduce((total, files) => total + files.length, 0);
}

function refreshedTimestamp(current, wallClockMs) {
  return Math.max(wallClockMs, current.mtimeMs + 0.001) / 1000;
}

function refreshSourceTimes(roots, wallClockMs) {
  for (const root of roots) {
    for (const relative of sourceFiles(root)) {
      const absolute = regularSource(root, relative);
      if (!absolute) continue;
      const stat = statSync(absolute);
      utimesSync(absolute, stat.atime, refreshedTimestamp(stat, wallClockMs));
    }
  }
}

function validSnapshot(snapshot, rootCount) {
  const validSource = (file) => file && typeof file === 'object'
    && typeof file.path === 'string' && typeof file.hash === 'string'
    && Number.isInteger(file.executable) && Number.isFinite(file.mtimeMs);
  const validSymlink = (link) => link && typeof link === 'object'
    && typeof link.path === 'string' && typeof link.target === 'string';
  return snapshot && typeof snapshot === 'object' && !Array.isArray(snapshot)
    && snapshot.version === 2
    && Array.isArray(snapshot.sources) && snapshot.sources.length === rootCount
    && snapshot.sources.every((files) => Array.isArray(files) && files.every(validSource))
    && Array.isArray(snapshot.symlinks) && snapshot.symlinks.length === rootCount
    && snapshot.symlinks.every((links) => Array.isArray(links) && links.every(validSymlink));
}

export function restoreSourceTimes(roots, source) {
  // Date.now() can precede a freshly written file on filesystems retaining
  // submillisecond timestamps. Use the high-resolution epoch clock and advance
  // by one microsecond only when the input is already newer.
  const wallClockMs = performance.timeOrigin + performance.now();
  if (!existsSync(source)) {
    refreshSourceTimes(roots, wallClockMs);
    return 0;
  }
  let snapshot;
  try {
    snapshot = JSON.parse(readFileSync(source, 'utf8'));
  } catch {
    refreshSourceTimes(roots, wallClockMs);
    return 0;
  }
  if (!validSnapshot(snapshot, roots.length)) {
    refreshSourceTimes(roots, wallClockMs);
    return 0;
  }
  let restored = 0;
  roots.forEach((root, index) => {
    const files = sourceFiles(root);
    // Cargo follows symlinks when checking dependency mtimes. Restoring a new
    // target's old timestamp could otherwise hide changed include_str! bytes.
    if (JSON.stringify(sourceSymlinks(root, files)) !== JSON.stringify(snapshot.symlinks[index])) {
      for (const file of files) {
        const absolute = regularSource(root, file);
        if (!absolute) continue;
        const stat = statSync(absolute);
        utimesSync(absolute, stat.atime, refreshedTimestamp(stat, wallClockMs));
      }
      return;
    }
    const tracked = new Set(files);
    const previous = new Map(snapshot.sources[index].map((file) => [file.path, file]));
    for (const relative of tracked) {
      const file = previous.get(relative);
      const absolute = regularSource(root, relative);
      if (!absolute) continue;
      const stat = statSync(absolute);
      if (!file || !Number.isFinite(file.mtimeMs)
        || contentHash(absolute) !== file.hash || (stat.mode & 0o111) !== file.executable) {
        // A fallback target archive can be newer than a fresh checkout. Make
        // changed and newly tracked inputs newer than the restored artifacts
        // so Cargo cannot accept stale fingerprints before rustc runs.
        utimesSync(absolute, stat.atime, refreshedTimestamp(stat, wallClockMs));
        continue;
      }
      utimesSync(absolute, stat.atime, file.mtimeMs / 1000);
      restored += 1;
    }
  });
  return restored;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const roots = JSON.parse(process.env.HYPERCOLOR_CACHE_SOURCE_ROOTS);
  const snapshot = process.env.HYPERCOLOR_CACHE_SOURCE_TIMES;
  const operation = process.argv[2];
  if (operation === 'capture') {
    console.log(`Recorded content and timestamps for ${captureSourceTimes(roots, snapshot)} source files`);
  } else if (operation === 'restore') {
    console.log(`Restored timestamps for ${restoreSourceTimes(roots, snapshot)} unchanged source files`);
  } else {
    throw new Error('Source timestamp operation must be capture or restore');
  }
}
