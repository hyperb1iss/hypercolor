#!/usr/bin/env node
// Render the Homebrew formula for a stable Hypercolor release.
//
// The public tag lane only ships Linux tarballs, so this fills the Linux
// stanzas from the release it just published and carries the macOS stanzas
// forward from the formula already in the tap. The signed macOS lane replaces
// those stanzas when it promotes an accepted build. Architectures the tap has
// never published are left out rather than pointed at artifacts that do not
// exist.
//
//   node scripts/homebrew-formula.mjs \
//     --version 0.5.0 \
//     --linux-amd64 <sha256> --linux-arm64 <sha256> \
//     --template packaging/homebrew/hypercolor.rb \
//     --current homebrew-tap/Formula/hypercolor.rb \
//     --output hypercolor.rb

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const VERSION_PATTERN = /^\d+\.\d+\.\d+$/;
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const MACOS_ARCHES = [
  { arch: 'arm64', placeholder: 'SHA256_MACOS_ARM64', guard: 'Hardware::CPU.arm?' },
  { arch: 'amd64', placeholder: 'SHA256_MACOS_AMD64', guard: 'Hardware::CPU.intel?' },
];
const LINUX_SHAS = { amd64: 'SHA256_LINUX_AMD64', arm64: 'SHA256_LINUX_ARM64' };

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

function requirePlaceholder(template, placeholder) {
  if (!template.includes(placeholder)) {
    throw new FormulaError(`template is missing the ${placeholder} placeholder`);
  }
}

function renderMacos(template, macos) {
  const block = macosBlock(template);
  if (block === undefined) throw new FormulaError('template has no on_macos block');
  const whole = `  on_macos do\n${block}  end\n`;
  if (macos === undefined) {
    // Drop the artifact block and the blank line that followed it.
    return template.replace(`${whole}\n`, '').replace(whole, '');
  }
  let rendered = block.replace('MACOS_VERSION_PLACEHOLDER', macos.version);
  const present = MACOS_ARCHES.filter(({ arch }) => macos.shas[arch] !== undefined);
  const branches = present.map(({ arch, placeholder, guard }, index) => {
    const keyword = index === 0 ? 'if' : 'elsif';
    return `    ${keyword} ${guard}\n      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-${arch}.tar.gz"\n      sha256 "${macos.shas[arch]}"\n`;
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
 * the value of readCurrentMacos for the formula already in the tap, or
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
    formula = formula.replace(placeholder, requireSha(linux[arch], `linux ${arch} sha256`));
  }
  const leftover = formula.match(/[A-Z0-9_]*PLACEHOLDER|SHA256_[A-Z0-9_]+/);
  if (leftover) throw new FormulaError(`${leftover[0]} survived rendering`);
  return formula;
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

export function main(argv) {
  const args = parseArgs(argv);
  for (const required of ['version', 'linux-amd64', 'linux-arm64', 'template', 'output']) {
    if (args[required] === undefined) throw new FormulaError(`--${required} is required`);
  }
  const template = readFileSync(args.template, 'utf8');
  const current = args.current !== undefined && existsSync(args.current)
    ? readCurrentMacos(readFileSync(args.current, 'utf8'))
    : undefined;
  const formula = renderFormula({
    template,
    version: args.version,
    linux: { amd64: args['linux-amd64'], arm64: args['linux-arm64'] },
    macos: current,
  });
  writeFileSync(args.output, formula);
  const carried = current === undefined
    ? 'no macOS stanzas carried (the tap has never published a macOS tarball)'
    : `macOS stanzas carried at ${current.version} for ${Object.keys(current.shas).join(', ')}`;
  console.log(`wrote ${args.output} for ${args.version}; ${carried}`);
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
