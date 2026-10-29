import { existsSync, readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const root = new URL('../', import.meta.url);
for (const name of ['dist/cli.js', 'native/engine.exe', 'README.md', 'NOTICE', 'LICENSE', 'LICENSE-APACHE-2.0', 'THIRD-PARTY-NOTICES', 'docs/validation.md']) {
  if (!existsSync(new URL(name, root))) throw new Error(`缺少发行文件：${name}，请先完成构建`);
}
const result = spawnSync(fileURLToPath(new URL('native/engine.exe', root)), ['--version'], { encoding: 'utf8', windowsHide: true });
if (result.status !== 0) throw new Error('Rust 核心无法启动');
const manifest = JSON.parse(readFileSync(new URL('package.json', root), 'utf8'));
if (!result.stdout.includes(manifest.version)) throw new Error('TS 与 Rust 的版本不一致');
