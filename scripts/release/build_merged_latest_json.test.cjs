const assert = require('assert/strict');
const { execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const test = require('node:test');

const scriptPath = path.resolve(__dirname, 'build_merged_latest_json.cjs');

function writeAsset(dir, name) {
  fs.writeFileSync(path.join(dir, name), 'asset');
  fs.writeFileSync(path.join(dir, `${name}.sig`), `signature:${name}`);
}

function createFixture(macAssets, options = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'latest-json-'));
  const assetsDir = path.join(dir, 'assets');
  const notesFile = path.join(dir, 'release-notes.md');
  const outputFile = path.join(dir, 'latest.json');
  fs.mkdirSync(assetsDir);
  fs.writeFileSync(notesFile, 'Release notes');

  const assets = [
    ...macAssets,
    'Cockpit Tools_0.20.51_x64-setup.exe',
    'cockpit-tools_0.20.51_amd64.AppImage',
    'cockpit-tools_0.20.51_aarch64.AppImage',
    'cockpit-tools_0.20.51_amd64.deb',
    'cockpit-tools_0.20.51_arm64.deb',
    'cockpit-tools-0.20.51-1.x86_64.rpm',
    'cockpit-tools-0.20.51-1.aarch64.rpm',
  ];

  if (options.includeMsi !== false) {
    assets.push('Cockpit Tools_0.20.51_x64_en-US.msi');
  }

  assets.forEach((name) => writeAsset(assetsDir, name));

  return { dir, assetsDir, notesFile, outputFile };
}

function runFixture(fixture) {
  execFileSync(process.execPath, [
    scriptPath,
    '--version',
    '0.20.51',
    '--repo',
    'owner/repo',
    '--assets-dir',
    fixture.assetsDir,
    '--notes-file',
    fixture.notesFile,
    '--published-at',
    '2026-05-26T13:40:34Z',
    '--output',
    fixture.outputFile,
  ]);

  return JSON.parse(fs.readFileSync(fixture.outputFile, 'utf8'));
}

test('builds latest.json for current macOS target-qualified updater names', () => {
  const latest = runFixture(
    createFixture([
      'Cockpit Tools_0.20.51_aarch64-apple-darwin.app.tar.gz',
      'Cockpit Tools_0.20.51_x86_64-apple-darwin.app.tar.gz',
    ])
  );

  assert.equal(latest.platforms['darwin-aarch64'].signature.includes('aarch64-apple-darwin'), true);
  assert.equal(latest.platforms['darwin-x86_64'].signature.includes('x86_64-apple-darwin'), true);
  assert.match(latest.platforms['darwin-aarch64'].url, /aarch64-apple-darwin\.app\.tar\.gz$/);
  assert.equal(Object.keys(latest.platforms).length, 15);
});

test('keeps compatibility with previous macOS updater names', () => {
  const latest = runFixture(
    createFixture([
      'Cockpit Tools_0.20.51_aarch64.app.tar.gz',
      'Cockpit Tools_0.20.51_x64.app.tar.gz',
    ])
  );

  assert.match(latest.platforms['darwin-aarch64'].url, /_aarch64\.app\.tar\.gz$/);
  assert.match(latest.platforms['darwin-x86_64'].url, /_x64\.app\.tar\.gz$/);
});

test('builds latest.json when Windows MSI is absent', () => {
  const latest = runFixture(
    createFixture(
      [
        'Cockpit Tools_0.20.51_aarch64-apple-darwin.app.tar.gz',
        'Cockpit Tools_0.20.51_x86_64-apple-darwin.app.tar.gz',
      ],
      { includeMsi: false }
    )
  );

  assert.equal(latest.platforms['windows-x86_64-msi'], undefined);
  assert.match(latest.platforms['windows-x86_64'].url, /_x64-setup\.exe$/);
  assert.match(latest.platforms['windows-x86_64-nsis'].url, /_x64-setup\.exe$/);
  assert.equal(Object.keys(latest.platforms).length, 14);
});
