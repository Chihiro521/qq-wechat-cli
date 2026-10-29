import { createWriteStream } from 'node:fs';
import { access, link, mkdir, rename, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import type { Writable } from 'node:stream';
import { finished } from 'node:stream/promises';
import type { Frame, Message } from './types.js';

export async function write(stream: Writable, text: string): Promise<void> {
  if (!stream.write(text)) await once(stream, 'drain');
}

export async function exportMessages(frames: AsyncIterable<Frame>, options: Record<string, unknown>): Promise<Record<string, unknown>> {
  const format = String(options.format ?? 'markdown');
  if (!['markdown', 'txt', 'text', 'jsonl', 'chatlab'].includes(format)) throw new Error(`INVALID_ARGUMENT: 不支持的导出格式 ${format}`);
  const target = options.output ? resolve(String(options.output)) : undefined;
  const temporary = target ? `${target}.${randomUUID()}.tmp` : undefined;
  if (target && !options.force) {
    let exists = false;
    try { await access(target); exists = true; } catch {}
    if (exists) throw new Error('OUTPUT_EXISTS: 文件已存在，使用 --force 覆盖');
  }
  let stream: Writable = process.stdout;
  let opened = false;
  let started = false;
  let streamError: Error | undefined;
  const observeError = (error: Error) => { streamError = error; };
  let count = 0;
  let meta: Record<string, unknown> = {};
  try {
    for await (const frame of frames) {
      if (streamError) throw streamError;
      if (frame.kind === 'start') {
        if (started) throw new Error('PROTOCOL_MISMATCH: 重复的导出起始帧');
        started = true;
        // The engine validates protected paths before it emits this frame.
        if (temporary) {
          await mkdir(dirname(target!), { recursive: true });
          stream = createWriteStream(temporary, { flags: 'wx', encoding: 'utf8' });
          opened = true;
        }
        stream.on('error', observeError);
        meta = frame.meta ?? {};
        if (format === 'jsonl') await write(stream, JSON.stringify({ _type: 'header', format: 'qq-wechat-cli-jsonl', version: 1, ...meta }) + '\n');
        if (format === 'chatlab') await write(stream, JSON.stringify({ _type: 'header', chatlab: { version: '0.0.2', exportedAt: Math.floor(Date.now() / 1000), generator: 'qq-wechat-cli' }, meta: { name: meta.conversationName, platform: meta.platform, type: meta.conversationKind === 'group' ? 'group' : 'private', groupId: meta.conversationKind === 'group' ? meta.peerId : null, peerId: meta.conversationKind === 'group' ? null : meta.peerId, sourceSessionId: meta.sourceSessionId } }) + '\n');
        if (format === 'markdown') await write(stream, `# ${meta.conversationName ?? '聊天记录'}\n\n`);
      }
      if (frame.kind === 'member' && format === 'chatlab') {
        if (!started) throw new Error('PROTOCOL_MISMATCH: 缺少导出起始帧');
        await write(stream, JSON.stringify({ _type: 'member', ...(frame.data as Record<string, unknown>) }) + '\n');
      }
      if (frame.kind === 'end') meta = { ...meta, ...frame.meta };
      if (frame.kind !== 'row') continue;
      if (!started) throw new Error('PROTOCOL_MISMATCH: 缺少导出起始帧');
      const message = frame.data as Message;
      if (format === 'jsonl') await write(stream, JSON.stringify(message) + '\n');
      else if (format === 'chatlab') {
        await write(stream, JSON.stringify({ _type: 'message', sender: message.senderId, accountName: message.senderName, groupNickname: message.senderName, timestamp: message.time, type: 0, content: message.text, platformMessageId: message.originalId }) + '\n');
      } else {
        const stamp = new Date(message.time * 1000).toISOString();
        const label = `[${stamp}] ${message.senderName}`;
        await write(stream, format === 'markdown' ? `**${label.replaceAll('*', '\\*')}**\n\n${message.text}\n\n` : `${label}: ${message.text}\n`);
      }
      count++;
    }
    if (!started) throw new Error('PROTOCOL_MISMATCH: 缺少导出起始帧');
    if (temporary && opened) {
      const complete = finished(stream);
      stream.end();
      await complete;
      if (streamError) throw streamError;
      if (!options.force) {
        // Hard-link publication is atomic and never replaces a concurrently created file.
        try { await link(temporary, target!); }
        catch (error) {
          if ((error as NodeJS.ErrnoException).code === 'EEXIST') throw new Error('OUTPUT_EXISTS: 导出期间目标文件已被创建');
          throw error;
        }
        await rm(temporary);
      } else {
        await rename(temporary, target!);
      }
    }
    return { count, output: target ?? 'stdout', format, ...meta, nodePeakMemoryBytes: process.resourceUsage().maxRSS * 1024 };
  } catch (error) {
    if (temporary && opened) {
      stream.destroy();
      await finished(stream).catch(() => {});
      await rm(temporary, { force: true }).catch(() => {});
    }
    throw error;
  } finally {
    stream.removeListener('error', observeError);
  }
}
