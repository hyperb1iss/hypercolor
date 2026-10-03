#!/usr/bin/env node
// Render the Homebrew formula for a stable Hypercolor release.
//
// Linux checksums always come from the release being published. macOS takes
// one of two complete forms: the release's own Apple silicon tarball and DMG,
// which renders the formula and the cask together, or no macOS input at all,
// which carries the macOS stanza forward from the formula already in the tap
// and leaves the cask alone. A release that is missing only part of its macOS
// assets never renders. macOS ships for Apple silicon only, so there is no
// Intel macOS input.
//
//   node scripts/homebrew-formula.mjs \
//     --version 0.5.0 \
//     --linux-amd64 <sha256> --linux-arm64 <sha256> \
//     --template packaging/homebrew/hypercolor.rb \
//     --macos-arm64 <sha256> --dmg-arm64 <sha256> \
//     --cask-template packaging/homebrew/hypercolor-app.rb \
//     --output hypercolor.rb --cask-output hypercolor-app.rb
//
//   node scripts/homebrew-formula.mjs \
//     --version 0.5.1 \
//     --linux-amd64 <sha256> --linux-arm64 <sha256> \
//     --template packaging/homebrew/hypercolor.rb \
//     --carry-macos-from homebrew-tap/Formula/hypercolor.rb \
//     --output hypercolor.rb

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const VERSION_PATTERN = /^\d+\.\d+\.\d+$/;
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const MACOS_SHAS = { arm64: 'SHA256_MACOS_ARM64' };
const LINUX_SHAS = { amd64: 'SHA256_LINUX_AMD64', arm64: 'SHA256_LINUX_ARM64' };
const REQUIRED_OPTIONS = ['version', 'linux-amd64', 'linux-arm64', 'template', 'output'];
const MACOS_RELEASE_OPTIONS = ['macos-arm64', 'dmg-arm64', 'cask-template', 'cask-output'];
const CARRY_OPTION = 'carry-macos-from';
const CLI_OPTIONS = [...REQUIRED_OPTIONS, ...MACOS_RELEASE_OPTIONS, CARRY_OPTION];
const MACOS_BLOCK_PATTERN = /^  on_macos do\n([\s\S]*?)^  end\n/m;
const LEFTOVER_PATTERN = /[A-Z0-9_]*PLACEHOLDER|SHA256_[A-Z0-9_]+/;

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

/**
 * Render every architecture. `macos` holds this release's checksums, or a
 * `carried` stanza read from the published formula by `readPublishedMacos`.
 */
export function renderFormula({ template, version, linux, macos }) {
  requireVersion(version, 'version');
  for (const placeholder of ['VERSION_PLACEHOLDER', ...Object.values(LINUX_SHAS), ...Object.values(MACOS_SHAS)]) {
    requirePlaceholder(template, placeholder);
  }
  let formula = template.replace('VERSION_PLACEHOLDER', version);
  if (macos?.carried) {
    const carried = macos.carried;
    requireVersion(carried.version, 'carried macOS version');
    const block = formula.match(MACOS_BLOCK_PATTERN);
    if (!block || !block[1].includes(MACOS_SHAS.arm64)) {
      throw new FormulaError('template has no on_macos download block to carry forward into');
    }
    // A block-level version points the macOS URL at the carried release.
    formula = formula.replace(block[0],
      block[0].replace('  on_macos do\n', `  on_macos do\n    version "${carried.version}"\n`));
    macos = { arm64: carried.arm64 };
  }
  for (const [platform, checksums, placeholders] of [
    ['linux', linux, LINUX_SHAS], ['macos', macos, MACOS_SHAS],
  ]) {
    for (const [arch, placeholder] of Object.entries(placeholders)) {
      formula = formula.replace(placeholder, requireSha(checksums?.[arch], `${platform} ${arch} sha256`));
    }
  }
  const leftover = formula.match(LEFTOVER_PATTERN);
  if (leftover) throw new FormulaError(`${leftover[0]} survived rendering`);
  return formula;
}

/** Read the macOS version and checksum the published formula installs. */
export function readPublishedMacos(formula) {
  const block = formula.match(MACOS_BLOCK_PATTERN)?.[1];
  if (block === undefined) throw new FormulaError('published formula has no on_macos download block');
  const version = block.match(/^    version "([^"]+)"$/m)?.[1] ?? formula.match(/^  version "([^"]+)"$/m)?.[1];
  const checksums = [...block.matchAll(/sha256 "([^"]+)"/g)].map(match => match[1]);
  if (checksums.length !== 1) {
    throw new FormulaError(`published on_macos block must carry exactly one sha256, found ${checksums.length}`);
  }
  return {
    version: requireVersion(version, 'published macOS version'),
    arm64: requireSha(checksums[0], 'published macOS arm64 sha256'),
  };
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (!flag.startsWith('--')) throw new FormulaError(`unexpected argument ${flag}`);
    if (!CLI_OPTIONS.includes(flag.slice(2))) throw new FormulaError(`unknown option ${flag}`);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith('--')) throw new FormulaError(`${flag} needs a value`);
    args[flag.slice(2)] = value;
    index += 1;
  }
  return args;
}

export function main(argv) {
  const args = parseArgs(argv);
  for (const required of REQUIRED_OPTIONS) {
    if (args[required] === undefined) throw new FormulaError(`--${required} is required`);
  }
  const macosGiven = MACOS_RELEASE_OPTIONS.filter(option => args[option] !== undefined);
  const carry = args[CARRY_OPTION];
  if (carry !== undefined && macosGiven.length > 0) {
    throw new FormulaError(`--${CARRY_OPTION} cannot be combined with --${macosGiven[0]}`);
  }
  if (carry === undefined && macosGiven.length !== MACOS_RELEASE_OPTIONS.length) {
    const missing = MACOS_RELEASE_OPTIONS.find(option => args[option] === undefined);
    throw new FormulaError(`--${missing} is required unless --${CARRY_OPTION} names the published formula`);
  }
  const linux = { amd64: args['linux-amd64'], arm64: args['linux-arm64'] };
  const template = readFileSync(args.template, 'utf8');
  if (carry !== undefined) {
    const carried = readPublishedMacos(readFileSync(carry, 'utf8'));
    const formula = renderFormula({ template, version: args.version, linux, macos: { carried } });
    writeFileSync(args.output, formula);
    console.log(`wrote formula for ${args.version}; macOS stays on ${carried.version} and the cask is unchanged`);
    return;
  }
  const formula = renderFormula({ template, version: args.version, linux, macos: { arm64: args['macos-arm64'] } });
  const cask = renderCask({ template: readFileSync(args['cask-template'], 'utf8'),
    version: args.version, arm64: args['dmg-arm64'] });
  writeFileSync(args.output, formula);
  writeFileSync(args['cask-output'], cask);
  console.log(`wrote formula and cask for ${args.version}`);
}

export function renderCask({ template, version, arm64 }) {
  requireVersion(version, 'version');
  const values = { VERSION_PLACEHOLDER: version,
    SHA256_MACOS_APP_ARM64: requireSha(arm64, 'DMG arm64 sha256') };
  for (const [placeholder, value] of Object.entries(values)) {
    requirePlaceholder(template, placeholder);
    template = template.replace(placeholder, value);
  }
  const leftover = template.match(LEFTOVER_PATTERN);
  if (leftover) throw new FormulaError(`${leftover[0]} survived rendering`);
  return template;
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
