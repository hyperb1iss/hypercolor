#!/usr/bin/env node
// Render the Homebrew formula for a stable Hypercolor release.
//
// Formula and cask metadata come from the same published release. Every
// platform checksum is required so a partial release cannot advance the tap.
//
//   node scripts/homebrew-formula.mjs \
//     --version 0.5.0 \
//     --linux-amd64 <sha256> --linux-arm64 <sha256> \
//     --template packaging/homebrew/hypercolor.rb \
//     --macos-amd64 <sha256> --macos-arm64 <sha256> \
//     --dmg-arm64 <sha256> --dmg-x86_64 <sha256> \
//     --cask-template packaging/homebrew/hypercolor-app.rb \
//     --output hypercolor.rb --cask-output hypercolor-app.rb

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const VERSION_PATTERN = /^\d+\.\d+\.\d+$/;
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const MACOS_SHAS = { amd64: 'SHA256_MACOS_AMD64', arm64: 'SHA256_MACOS_ARM64' };
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

function requirePlaceholder(template, placeholder) {
  if (!template.includes(placeholder)) {
    throw new FormulaError(`template is missing the ${placeholder} placeholder`);
  }
}

/** Render all architectures from the checksums of one published release. */
export function renderFormula({ template, version, linux, macos }) {
  requireVersion(version, 'version');
  for (const placeholder of ['VERSION_PLACEHOLDER', ...Object.values(LINUX_SHAS), ...Object.values(MACOS_SHAS)]) {
    requirePlaceholder(template, placeholder);
  }
  let formula = template.replace('VERSION_PLACEHOLDER', version);
  for (const [platform, checksums, placeholders] of [
    ['linux', linux, LINUX_SHAS], ['macos', macos, MACOS_SHAS],
  ]) {
    for (const [arch, placeholder] of Object.entries(placeholders)) {
      formula = formula.replace(placeholder, requireSha(checksums?.[arch], `${platform} ${arch} sha256`));
    }
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
  for (const required of ['version', 'linux-amd64', 'linux-arm64', 'macos-amd64', 'macos-arm64',
    'dmg-arm64', 'dmg-x86_64', 'template', 'output', 'cask-template', 'cask-output']) {
    if (args[required] === undefined) throw new FormulaError(`--${required} is required`);
  }
  const template = readFileSync(args.template, 'utf8');
  const macos = {
    amd64: requireSha(args['macos-amd64'], 'macOS amd64 sha256'),
    arm64: requireSha(args['macos-arm64'], 'macOS arm64 sha256'),
  };
  const formula = renderFormula({
    template,
    version: args.version,
    linux: { amd64: args['linux-amd64'], arm64: args['linux-arm64'] },
    macos,
  });
  const cask = renderCask({ template: readFileSync(args['cask-template'], 'utf8'),
    version: args.version, arm64: args['dmg-arm64'], x86_64: args['dmg-x86_64'] });
  writeFileSync(args.output, formula);
  writeFileSync(args['cask-output'], cask);
  console.log(`wrote formula and cask for ${args.version}`);
}

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
