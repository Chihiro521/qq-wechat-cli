import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, dirname } from 'node:path';
import { spawnSync } from 'node:child_process';
export const root = fileURLToPath(new URL('../', import.meta.url));
export const cli = join(root, 'dist/cli.js');
export function workspace() {
  mkdirSync(join(root, '.cache/tests'), { recursive: true });
  const dir = mkdtempSync(join(root, '.cache/tests/case-'));
  return { dir, home: join(dir, 'state'), close() { rmSync(dir, { recursive: true, force: true, maxRetries: 3 }); } };
}
export function run(home, platform, command, ...args) {
  const result = spawnSync(process.execPath, [cli, platform, command, ...args, '--data-dir', home], {
    cwd: root, encoding: 'utf8', timeout: 60000, maxBuffer: 32 * 1024 * 1024,
    env: { ...process.env, NODE_NO_WARNINGS: '1' }, windowsHide: true,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(result.stderr || result.stdout || `exit ${result.status}`);
  return JSON.parse(result.stdout);
}
export function fail(home, platform, command, ...args) {
  return spawnSync(process.execPath, [cli, platform, command, ...args, '--data-dir', home], { cwd: root, encoding: 'utf8', env: { ...process.env, NODE_NO_WARNINGS: '1' }, windowsHide: true, timeout: 60000 });
}
export function qqFixture(dir, encrypted = false) {
  const fixture = JSON.parse(readFileSync(new URL('./fixtures/qq.json', import.meta.url), 'utf8'));
  const path = join(dir, 'Msg3.0.db'); writeFileSync(path, Buffer.from(fixture[encrypted ? 'encrypted' : 'plain'], 'hex'));
  const key = join(dir, 'qq-keys.json'); writeFileSync(key, JSON.stringify({ qq: '42'.repeat(16) }));
  return { path, key, fixture };
}
export function wxFixture(dir, encrypted = true) {
  const fixture = JSON.parse(readFileSync(new URL('./fixtures/wechat.json', import.meta.url), 'utf8'));
  const path = join(dir, 'wxid_fixture/db_storage'); const keys = {};
  for (const file of fixture.files) {
    const full = join(path, file.path); mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, Buffer.from(file[encrypted ? 'encrypted' : 'plain'], 'hex'));
    keys[file.path] = { enc_key: '24'.repeat(32), salt: file.salt };
  }
  const key = join(dir, 'wx-keys.json'); writeFileSync(key, JSON.stringify(keys));
  return { path, key, fixture };
}
