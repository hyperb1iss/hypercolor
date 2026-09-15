#!/usr/bin/env node
// Render the Homebrew formula and cask for a stable Hypercolor release.
//
// Linux stanzas always come from the release that was just published. macOS
// stanzas come from the same release when it shipped signed macOS artifacts,
// and are otherwise carried forward from the formula already in the tap, so
// a Linux-only tag never points macOS users at artifacts that do not exist.
// A release that publishes only part of the macOS set is rejected: the tap
// advances all of macOS or none of it.
//
//   node scripts/homebrew-formula.mjs \
//     --version 0.5.2 \
//     --linux-amd64 <sha256> --linux-arm64 <sha256> \
//     --template packaging/homebrew/hypercolor.rb \
//     --current homebrew-tap/Formula/hypercolor.rb \
//     --output homebrew-tap/Formula/hypercolor.rb \
//     [--macos-amd64 <sha256> --macos-arm64 <sha256> \
//      --dmg-arm64 <sha256> --dmg-x86_64 <sha256> \
//      --cask-template packaging/homebrew/hypercolor-app.rb \
//      --cask-output homebrew-tap/Casks/hypercolor-app.rb]

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const VERSION_PATTERN = /^\d+\.\d+\.\d+$/;
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const MACOS_ARCHES = [
  { arch: 'arm64', placeholder: 'SHA256_MACOS_ARM64', guard: 'Hardware::CPU.arm?' },
  { arch: 'amd64', placeholder: 'SHA256_MACOS_AMD64', guard: 'Hardware::CPU.intel?' },
];
const LINUX_SHAS = { amd64: 'SHA256_LINUX_AMD64', arm64: 'SHA256_LINUX_ARM64' };
const MACOS_FLAGS = ['macos-amd64', 'macos-arm64', 'dmg-arm64', 'dmg-x86_64', 'cask-template', 'cask-output'];

class FormulaError extends Error {}

function requireVersion(value, label) {
  if (!VERSION_PATTERN.test(value ?? '')) {
    throw new FormulaError(`${label} must be a stable X.Y.Z version, got ${JSON.stringify(value ?? null)}`);
  }
  return value;
}

function requireSha(value, label) {
  if (!SHA256_PATTERN.test(value ?? '')) {
    throw new FormulaError(`${label} must be a lowercase hex sha256, got ${JSON.stringify(value ?? null)}`);
  }
  return value;
}

function requirePlaceholder(template, placeholder) {
  if (!template.includes(placeholder)) {
    throw new FormulaError(`template is missing the ${placeholder} placeholder`);
  }
}

function macosBlock(formula) {
  const match = formula.match(/^  on_macos do\n([\s\S]*?)^  end\n/m);
  return match ? match[1] : undefined;
}

/**
 * Read the macOS stanzas a published formula carries.
 *
 * Returns undefined when the formula has no on_macos block with a tarball
 * url, otherwise the version and every architecture sha it publishes.
 */
export function readCurrentMacos(formula) {
  const block = macosBlock(formula);
  if (block === undefined) return undefined;
  const shas = {};
  for (const { arch } of MACOS_ARCHES) {
    const pattern = new RegExp(`hypercolor-[^"\\n]*-macos-${arch}\\.tar\\.gz"\\s*\\n\\s*sha256 "([0-9a-f]{64})"`);
    const found = block.match(pattern);
    if (found) shas[arch] = found[1];
  }
  if (Object.keys(shas).length === 0) return undefined;
  const blockVersion = block.match(/^\s*version "([^"]+)"/m)?.[1];
  const topVersion = formula.match(/^  version "([^"]+)"/m)?.[1];
  const version = requireVersion(blockVersion ?? topVersion, 'current macOS version');
  return { version, shas };
}

function renderMacos(template, macos) {
  const block = macosBlock(template);
  if (block === undefined) throw new FormulaError('template has no on_macos block');
  const whole = `  on_macos do\n${block}  end\n`;
  if (macos === undefined) {
    // Drop the artifact block and the blank line that followed it.
    return template.replace(`${whole}\n`, '').replace(whole, '');
  }
  requireVersion(macos.version, 'macOS version');
  let rendered = block.replace('MACOS_VERSION_PLACEHOLDER', macos.version);
  const present = MACOS_ARCHES.filter(({ arch }) => macos.shas[arch] !== undefined);
  if (present.length === 0) throw new FormulaError('macOS stanzas need at least one architecture sha256');
  const branches = present.map(({ arch, guard }, index) => {
    const keyword = index === 0 ? 'if' : 'elsif';
    const sha = requireSha(macos.shas[arch], `macos ${arch} sha256`);
    return `    ${keyword} ${guard}\n      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-${arch}.tar.gz"\n      sha256 "${sha}"\n`;
  });
  const conditional = rendered.match(/^    if Hardware::CPU[\s\S]*?^    end\n/m);
  if (!conditional) throw new FormulaError('template on_macos block has no architecture conditional');
  rendered = rendered.replace(conditional[0], `${branches.join('')}    end\n`);
  for (const { placeholder } of MACOS_ARCHES) {
    if (rendered.includes(placeholder)) throw new FormulaError(`${placeholder} survived rendering`);
  }
  return template.replace(whole, `  on_macos do\n${rendered}  end\n`);
}

/**
 * Render the formula text.
 *
 * `linux` maps amd64 and arm64 to the published tarball digests. `macos` is
 * `{ version, shas }`: the release's own digests when it shipped macOS, the
 * value of readCurrentMacos for the tap's formula when it did not, or
 * undefined when the tap has never published a macOS build.
 */
export function renderFormula({ template, version, linux, macos }) {
  requireVersion(version, 'version');
  for (const placeholder of ['VERSION_PLACEHOLDER', 'MACOS_VERSION_PLACEHOLDER', ...Object.values(LINUX_SHAS)]) {
    requirePlaceholder(template, placeholder);
  }
  let formula = renderMacos(template, macos);
  formula = formula.replace('  version "VERSION_PLACEHOLDER"', `  version "${version}"`);
  for (const [arch, placeholder] of Object.entries(LINUX_SHAS)) {
    formula = formula.replace(placeholder, requireSha(linux?.[arch], `linux ${arch} sha256`));
  }
  const leftover = formula.match(/[A-Z0-9_]*PLACEHOLDER|SHA256_[A-Z0-9_]+/);
  if (leftover) throw new FormulaError(`${leftover[0]} survived rendering`);
  return formula;
}

/** Render the desktop app cask from both notarized DMG digests. */
export function renderCask({ template, version, arm64, x86_64 }) {
  requireVersion(version, 'version');
  const values = { VERSION_PLACEHOLDER: version,
    SHA256_MACOS_APP_ARM64: requireSha(arm64, 'DMG arm64 sha256'),
    SHA256_MACOS_APP_X86_64: requireSha(x86_64, 'DMG x86_64 sha256') };
  for (const [placeholder, value] of Object.entries(values)) {
    requirePlaceholder(template, placeholder);
    template = template.replace(placeholder, value);
  }
  return template;
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (!flag.startsWith('--')) throw new FormulaError(`unexpected argument ${flag}`);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith('--')) throw new FormulaError(`${flag} needs a value`);
    args[flag.slice(2)] = value;
    index += 1;
  }
  return args;
}

/**
 * Decide where the macOS stanzas come from.
 *
 * Every macOS flag present means the release shipped the complete signed
 * set and the tap advances with it. None present means carry the tap's
 * current stanzas forward. Anything in between is a partial release.
 */
function resolveMacos(args) {
  const given = MACOS_FLAGS.filter((flag) => args[flag] !== undefined);
  if (given.length === MACOS_FLAGS.length) {
    return {
      formula: { version: args.version, shas: { amd64: args['macos-amd64'], arm64: args['macos-arm64'] } },
      cask: { arm64: args['dmg-arm64'], x86_64: args['dmg-x86_64'] },
    };
  }
  if (given.length > 0) {
    const missing = MACOS_FLAGS.filter((flag) => args[flag] === undefined).map((flag) => `--${flag}`);
    throw new FormulaError(`partial macOS release: ${missing.join(', ')} missing; publish all macOS artifacts or none`);
  }
  const current = args.current !== undefined && existsSync(args.current)
    ? readCurrentMacos(readFileSync(args.current, 'utf8'))
    : undefined;
  return { formula: current, cask: undefined };
}

export function main(argv) {
  const args = parseArgs(argv);
  for (const required of ['version', 'linux-amd64', 'linux-arm64', 'template', 'output']) {
    if (args[required] === undefined) throw new FormulaError(`--${required} is required`);
  }
  const macos = resolveMacos(args);
  const formula = renderFormula({
    template: readFileSync(args.template, 'utf8'),
    version: args.version,
    linux: { amd64: args['linux-amd64'], arm64: args['linux-arm64'] },
    macos: macos.formula,
  });
  const cask = macos.cask === undefined ? undefined : renderCask({
    template: readFileSync(args['cask-template'], 'utf8'),
    version: args.version, arm64: macos.cask.arm64, x86_64: macos.cask.x86_64 });
  writeFileSync(args.output, formula);
  if (cask !== undefined) writeFileSync(args['cask-output'], cask);
  if (macos.cask !== undefined) {
    console.log(`wrote formula and cask for ${args.version}`);
  } else if (macos.formula === undefined) {
    console.log(`wrote formula for ${args.version}: Linux only, the tap has no macOS build to carry`);
  } else {
    const arches = Object.keys(macos.formula.shas).join(', ');
    console.log(`wrote formula for ${args.version}: macOS stanzas carried at ${macos.formula.version} for ${arches}`);
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    if (error instanceof FormulaError) {
      console.error(`homebrew-formula: ${error.message}`);
      process.exit(1);
    }
    throw error;
  }
}
