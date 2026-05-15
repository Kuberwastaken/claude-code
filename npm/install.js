#!/usr/bin/env node
'use strict';

const https = require('https');
const http = require('http');
const fs = require('fs');
const path = require('path');
const os = require('os');
const { execFileSync } = require('child_process');

const pkg = require('./package.json');
const VERSION = pkg.version;
const REPO = 'kuberwastaken/claurst';
const BASE_URL = `https://github.com/${REPO}/releases/download/v${VERSION}`;
const NATIVE_DIR = path.join(__dirname, 'native');

function getPlatform() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === 'win32' && arch === 'x64') {
    return { artifact: 'claurst-windows-x86_64', ext: '.exe', archive: '.zip' };
  }
  if (platform === 'linux' && arch === 'x64') {
    return { artifact: 'claurst-linux-x86_64', ext: '', archive: '.tar.gz' };
  }
  if (platform === 'linux' && arch === 'arm64') {
    return { artifact: 'claurst-linux-aarch64', ext: '', archive: '.tar.gz' };
  }
  if (platform === 'darwin' && arch === 'x64') {
    return { artifact: 'claurst-macos-x86_64', ext: '', archive: '.tar.gz' };
  }
  if (platform === 'darwin' && arch === 'arm64') {
    return { artifact: 'claurst-macos-aarch64', ext: '', archive: '.tar.gz' };
  }
  throw new Error(
    `Unsupported platform: ${platform}/${arch}.\n` +
    `Install manually from: https://github.com/${REPO}/releases/tag/v${VERSION}`
  );
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest);
    const get = url.startsWith('https') ? https : http;
    get.get(url, (res) => {
      if (res.statusCode === 301 || res.statusCode === 302) {
        file.close();
        try { fs.unlinkSync(dest); } catch (_) {}
        download(res.headers.location, dest).then(resolve).catch(reject);
        return;
      }
      if (res.statusCode !== 200) {
        file.close();
        try { fs.unlinkSync(dest); } catch (_) {}
        reject(new Error(`HTTP ${res.statusCode} downloading ${url}`));
        return;
      }
      res.pipe(file);
      file.on('finish', () => file.close(resolve));
      file.on('error', (err) => {
        try { fs.unlinkSync(dest); } catch (_) {}
        reject(err);
      });
    }).on('error', (err) => {
      try { fs.unlinkSync(dest); } catch (_) {}
      reject(err);
    });
  });
}

async function main() {
  const { artifact, ext, archive } = getPlatform();
  const archiveName = `${artifact}${archive}`;
  const url = `${BASE_URL}/${archiveName}`;
  const tmpPath = path.join(os.tmpdir(), `claurst-install-${process.pid}${archive}`);
  const binaryDest = path.join(NATIVE_DIR, `claurst${ext}`);

  if (fs.existsSync(binaryDest)) {
    console.log('claurst: native binary already present, skipping download.');
    return;
  }

  fs.mkdirSync(NATIVE_DIR, { recursive: true });

  console.log(`claurst: downloading v${VERSION} for ${process.platform}/${process.arch}`);
  console.log(`         ${url}`);
  await download(url, tmpPath);

  console.log('claurst: extracting...');
  if (archive === '.zip') {
    execFileSync('powershell', [
      '-NoProfile', '-NonInteractive', '-Command',
      `Expand-Archive -Force -Path "${tmpPath}" -DestinationPath "${NATIVE_DIR}"`
    ]);
  } else {
    execFileSync('tar', ['-xzf', tmpPath, '-C', NATIVE_DIR]);
  }

  try { fs.unlinkSync(tmpPath); } catch (_) {}

  if (!fs.existsSync(binaryDest)) {
    throw new Error(`Extraction succeeded but binary not found at ${binaryDest}`);
  }

  if (ext === '') {
    fs.chmodSync(binaryDest, 0o755);
  }

  console.log(`claurst: ready — run \`claurst\` to start.`);
}

main().catch((err) => {
  console.error(`\nclaurst install failed: ${err.message}`);
  console.error(`Manual install: https://github.com/${REPO}/releases/tag/v${VERSION}\n`);
  process.exit(1);
});
