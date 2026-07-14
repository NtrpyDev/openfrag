#!/usr/bin/env node
'use strict';

// PROTOTYPE TUI / one-shot runner. The metric logic is isolated in logic.js.

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const readline = require('readline');
const { execFileSync } = require('child_process');
const { analyseAimMetrics } = require('./logic');

const PINNED_COMMIT = 'ba39cc44cd5abfd7f34df2b3c0a7dd3630048311';
const DEFAULT_ROOTS = [
  '/tmp/openfrag-demoparser-audit',
  '/tmp/demoparser-upstream',
  '/tmp/demoparser',
];

function sha256File(filePath) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('hex');
}

function sorted(value) {
  if (Array.isArray(value)) return value.map(sorted);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sorted(value[key])]));
  }
  return value;
}

function stableJson(value, pretty = false) {
  return JSON.stringify(sorted(value), null, pretty ? 2 : 0);
}

function locateParserRoot() {
  if (process.env.DEMOPARSER_ROOT) return process.env.DEMOPARSER_ROOT;
  return DEFAULT_ROOTS.find((root) => fs.existsSync(path.join(root, 'src/node/index.js')));
}

function loadReport() {
  const parserRoot = locateParserRoot();
  if (!parserRoot) {
    throw new Error('Pinned demoparser checkout not found; set DEMOPARSER_ROOT. See README.md.');
  }

  const demoPath = process.env.AIM_DEMO_PATH || path.join(parserRoot, 'src/parser/test_demo.dem');
  const nodeBindingPath = path.join(parserRoot, 'src/node');
  const nativeBindingPath = path.join(nodeBindingPath, 'demoparser2.linux-x64-gnu.node');
  if (!fs.existsSync(demoPath)) throw new Error(`Demo fixture not found: ${demoPath}`);
  if (!fs.existsSync(nativeBindingPath)) throw new Error(`Built parser binding not found: ${nativeBindingPath}`);

  const parserCommit = execFileSync('git', ['-C', parserRoot, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  if (parserCommit !== PINNED_COMMIT) {
    throw new Error(`Expected demoparser ${PINNED_COMMIT}, found ${parserCommit}`);
  }

  const parser = require(nodeBindingPath);
  const header = parser.parseHeader(demoPath);
  const playerHurts = parser.parseEvent(demoPath, 'player_hurt');
  const weaponFires = parser.parseEvent(demoPath, 'weapon_fire');
  const fireBullets = parser.parseEvent(demoPath, 'fire_bullets');
  const playerBulletHits = parser.parseEvent(demoPath, 'player_bullet_hit');
  const roundEnds = parser.parseEvent(demoPath, 'round_end');
  const hurtTicks = [...new Set(playerHurts.map((event) => event.tick))].sort((a, b) => a - b);
  const tickRows = parser.parseTicks(
    demoPath,
    ['pitch', 'yaw', 'X', 'Y', 'Z', 'is_alive', 'team_num'],
    hurtTicks,
  );

  const source = {
    parser_repository: 'https://github.com/LaihoE/demoparser',
    parser_commit: parserCommit,
    parser_binding_sha256: sha256File(nativeBindingPath),
    fixture_repository_path: 'src/parser/test_demo.dem',
    fixture_origin_commit: '4131a4fc02fda291b22421c20e1ca33f149535a7',
    fixture_sha256: sha256File(demoPath),
    fixture_bytes: fs.statSync(demoPath).size,
    fixture_use: 'Upstream tracked test fixture; analysed locally and not redistributed by openfrag.',
    repository_license: 'MIT; see the pinned repository LICENSE.',
    map_name: header.map_name,
    demo_version_name: header.demo_version_name,
    network_protocol: header.network_protocol,
    completed_round_events: roundEnds.length,
  };

  const report = analyseAimMetrics({
    source,
    playerHurts,
    weaponFires,
    fireBullets,
    playerBulletHits,
    roundEnds,
    tickRows,
  });
  report.determinism = {
    canonical_payload_sha256: crypto.createHash('sha256').update(stableJson(report)).digest('hex'),
    serialization: 'UTF-8 JSON with recursively sorted object keys; array and event order retained.',
  };

  const serialized = stableJson(report);
  if (/7656119\d{10}/.test(serialized)) {
    throw new Error('Privacy invariant failed: a raw SteamID reached the report.');
  }
  return report;
}

function summary(report) {
  return {
    prototype: report.prototype,
    source: report.source,
    decisions: report.decisions,
    empirical: {
      crosshair: report.crosshair.empirical,
      time_to_damage: report.time_to_damage.empirical,
      spray: report.spray.empirical,
    },
    determinism: report.determinism,
  };
}

function renderTui(report) {
  const pages = [
    ['overview', summary(report)],
    ['crosshair', { ...report.crosshair, records: report.crosshair.records.slice(0, 3) }],
    ['time-to-damage', { ...report.time_to_damage, records: report.time_to_damage.records.slice(0, 3) }],
    ['spray', report.spray],
    ['receipts-and-invariants', {
      crosshair_receipt_inputs: report.crosshair.receipt_inputs,
      time_receipt_inputs: report.time_to_damage.receipt_inputs,
      spray_receipt_inputs_if_later_supported: report.spray.receipt_inputs_if_later_supported,
      testable_invariants: report.testable_invariants,
      determinism: report.determinism,
      full_json_command: 'node prototypes/aim-metrics/run.js --json',
    }],
  ];
  let pageIndex = 0;

  function draw() {
    console.clear();
    const [title, state] = pages[pageIndex];
    process.stdout.write(`\x1b[1mAim metrics v1 prototype: ${title}\x1b[0m\n`);
    process.stdout.write(`\x1b[2mPage ${pageIndex + 1}/${pages.length}; full records are available with --json.\x1b[0m\n\n`);
    process.stdout.write(`${stableJson(state, true)}\n\n`);
    process.stdout.write('\x1b[1m[1-5]\x1b[0m page  \x1b[1m[n/p]\x1b[0m next/previous  \x1b[1m[q]\x1b[0m quit\n');
  }

  readline.emitKeypressEvents(process.stdin);
  process.stdin.setRawMode(true);
  process.stdin.resume();
  process.stdin.on('keypress', (_text, key) => {
    if (key.ctrl && key.name === 'c' || key.name === 'q') {
      process.stdin.setRawMode(false);
      process.stdout.write('\n');
      process.exit(0);
    }
    if (/^[1-5]$/.test(key.sequence)) pageIndex = Number(key.sequence) - 1;
    if (key.name === 'n' || key.name === 'right') pageIndex = (pageIndex + 1) % pages.length;
    if (key.name === 'p' || key.name === 'left') pageIndex = (pageIndex + pages.length - 1) % pages.length;
    draw();
  });
  draw();
}

function main() {
  const report = loadReport();
  if (process.argv.includes('--json')) {
    process.stdout.write(`${stableJson(report, true)}\n`);
    return;
  }
  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    process.stdout.write(`${stableJson(summary(report), true)}\n`);
    return;
  }
  renderTui(report);
}

main();
