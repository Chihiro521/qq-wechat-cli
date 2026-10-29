import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { Frame } from './types.js';

export function enginePath(): string {
  const paths = [process.env.QQ_WECHAT_ENGINE,
    fileURLToPath(new URL('../native/engine.exe', import.meta.url)),
    fileURLToPath(new URL('../native/target/release/engine.exe', import.meta.url)),
    fileURLToPath(new URL('../native/target/debug/engine.exe', import.meta.url))];
  const found = paths.find((p): p is string => !!p && existsSync(p));
  if (!found) throw new Error('ENGINE_NOT_FOUND: 缺少 Rust 核心，请运行 npm run build');
  return found;
}

export async function* engine(request: Record<string, unknown>): AsyncGenerator<Frame> {
  const child = spawn(enginePath(), [], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
  let launchError: Error | undefined;
  child.on('error', error => { launchError = error; });
  const completion = new Promise<number | null>(resolve => child.on('close', resolve));
  child.stderr.on('data', (data: Buffer) => process.stderr.write(data));
  const interrupt = () => child.kill();
  process.once('SIGINT', interrupt);
  process.once('SIGTERM', interrupt);
  process.once('exit', interrupt);
  child.stdin.on('error', () => {});
  child.stdin.end(JSON.stringify({ protocol: 1, ...request, parentPid: process.pid }) + '\n');
  let ended = false;
  try {
    for await (const line of createInterface({ input: child.stdout, crlfDelay: Infinity })) {
      if (!line.trim()) continue;
      const frame = JSON.parse(line) as Frame;
      if (frame.protocol !== 1) throw new Error('PROTOCOL_MISMATCH: Rust 核心协议版本不兼容');
      if (frame.kind === 'result' || frame.kind === 'end') ended = true;
      if (frame.ok === false) throw new Error(`${frame.error?.code ?? 'ENGINE_ERROR'}: ${frame.error?.message ?? '操作失败'}`);
      yield frame;
    }
    const status = await completion;
    if (launchError) throw launchError;
    if (status !== 0 || !ended) throw new Error(`ENGINE_EXITED: 核心异常退出（${status ?? '已取消'}）`);
  } finally {
    process.removeListener('SIGINT', interrupt);
    process.removeListener('SIGTERM', interrupt);
    process.removeListener('exit', interrupt);
    if (child.exitCode === null) child.kill();
  }
}
