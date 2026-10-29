import { copyFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const root = new URL('../', import.meta.url);
await mkdir(new URL('native/', root), { recursive: true });
await copyFile(new URL('native/target/release/engine.exe', root), new URL('native/engine.exe', root));
console.log('已生成 native/engine.exe');
