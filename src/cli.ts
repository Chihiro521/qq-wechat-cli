#!/usr/bin/env node
import { parseArgs } from 'node:util';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { engine } from './engine.js';
import { exportMessages, write } from './export.js';

const version = (JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as { version: string }).version;
const commands = new Set(['init', 'refresh', 'status', 'sessions', 'history', 'search', 'stats', 'contacts', 'members', 'favorites', 'unread', 'new-messages', 'summarize', 'export', 'clean', 'import']);
const help = `qq-wechat-cli ${version} — 本地 QQ / 微信聊天记录\n\n用法：\n  qq-wechat-cli <qq|wechat> <命令> [参数]\n  qq-wechat-cli migrate --from <qq|wechat> [--config FILE]\n\n命令：\n  init / refresh / status       初始化、刷新和查看同步状态\n  sessions / history / search  会话、历史和搜索\n  stats / summarize            统计和整理总结素材\n  contacts / members           联系人和群成员\n  favorites / unread           收藏和未读\n  new-messages                 查看检查点后的新增消息\n  export / clean / import      导出、清理缓存、导入旧配置\n\n示例：\n  qq-wechat-cli qq init --db "D:\\QQ-data\\Msg3.0.db" --account 123456\n  qq-wechat-cli wechat import\n  qq-wechat-cli wechat refresh\n  qq-wechat-cli qq history group:123456 --limit 50\n  qq-wechat-cli wechat search "关键词" --days 7 --refresh\n  qq-wechat-cli qq export group:123456 --format chatlab --output chat.jsonl\n\n公共参数：\n  --data-dir DIR     独立状态目录（默认 %LOCALAPPDATA%\\qq-wechat-cli）\n  --format json|text 输出格式，默认 JSON；export 另支持 markdown/txt/jsonl/chatlab\n  --refresh          查询前刷新；普通查询直接使用缓存\n  --limit N / --offset N / --before CURSOR\n  --start-time TIME / --end-time TIME / --days N\n  --chat CHAT / --type TYPE / --sender ID / --media\n  --account ID       指定账号；配置多个账号时必须明确选择\n\n初始化可用 --db PATH（QQ）或 --db-dir DIR（微信）。\nexport --force 允许覆盖；clean --all 同时移除该账号的配置。\n`;

function render(data: unknown, meta: Record<string, unknown> | undefined): string {
  const obj = data as Record<string, unknown> | undefined;
  const rows = Array.isArray(data) ? data : obj?.messages ?? obj?.sessions ?? obj?.contacts ?? obj?.members ?? obj?.favorites;
  const sync = meta?.syncedAt ? `上次同步：${meta.syncedAt}\n` : '';
  if (Array.isArray(rows)) return sync + rows.map((row: Record<string, unknown>) => row.text !== undefined ? `[${row.sentAt ?? row.time ?? ''}] ${row.senderName ?? ''}: ${row.text}` : JSON.stringify(row)).join('\n') + '\n';
  return sync + JSON.stringify(data, null, 2) + '\n';
}

export async function main(argv = process.argv.slice(2)): Promise<number> {
  let outputFormat = 'json';
  try {
    if (argv.includes('--version') || argv.includes('-v')) { console.log(version); return 0; }
    if (!argv.length || argv.includes('--help') || argv.includes('-h')) { console.log(help); return 0; }
    let platform = argv[0];
    let command = argv[1];
    let tokens = argv.slice(2);
    if (platform === 'migrate') { command = 'import'; tokens = argv.slice(1); }
    const stringOptions = ['from', 'data-dir', 'config', 'db', 'db-dir', 'favorites-db', 'account', 'account-uin', 'keys-file', 'format', 'output', 'limit', 'offset', 'before', 'start-time', 'end-time', 'days', 'sender', 'query', 'detail', 'kind', 'order', 'top', 'chunk-size', 'prompt'];
    const options: Record<string, { type: 'string' | 'boolean'; multiple?: boolean }> = Object.fromEntries(stringOptions.map(name => [name, { type: 'string' }]));
    for (const name of ['force', 'all', 'refresh', 'media', 'dry-run']) options[name] = { type: 'boolean' };
    for (const name of ['chat', 'type']) options[name] = { type: 'string', multiple: true };
    const parsed = parseArgs({ args: tokens, options, allowPositionals: true, strict: true });
    const values: Record<string, unknown> = { ...parsed.values, positionals: parsed.positionals };
    if (platform === 'migrate') platform = String(values.from ?? '');
    if (!['qq', 'wechat'].includes(platform ?? '')) throw new Error('INVALID_ARGUMENT: 请指定 qq 或 wechat');
    if (!commands.has(command ?? '')) throw new Error(`UNKNOWN_COMMAND: 未知命令 ${command ?? ''}`);
    outputFormat = String(values.format ?? 'json');
    if (command !== 'export' && !['json', 'text'].includes(outputFormat)) throw new Error('INVALID_ARGUMENT: 输出格式应为 json 或 text');
    for (const name of ['limit', 'offset', 'top', 'chunk-size', 'days']) {
      if (values[name] !== undefined && (!/^\d+$/.test(String(values[name])) || !Number.isSafeInteger(Number(values[name])))) throw new Error(`INVALID_ARGUMENT: --${name} 应为非负整数`);
      if (values[name] !== undefined) values[name] = Number(values[name]);
    }
    const request = { platform, command, dataDir: values['data-dir'] ?? process.env.QQ_WECHAT_HOME, args: values };
    if (command === 'export') {
      const result = await exportMessages(engine(request), values);
      if (values.output) await write(process.stdout, JSON.stringify({ ok: true, data: result }) + '\n');
    } else {
      for await (const frame of engine(request)) {
        if (frame.kind === 'result') await write(process.stdout, outputFormat === 'text' ? render(frame.data, frame.meta) : JSON.stringify({ ok: true, data: frame.data, meta: frame.meta }) + '\n');
      }
    }
    return 0;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const match = /^([A-Z][A-Z_]+):\s*([\s\S]*)$/.exec(message);
    const payload = { code: match?.[1] ?? 'CLI_ERROR', message: match?.[2] ?? message };
    await write(process.stderr, outputFormat === 'text' ? `${payload.code}: ${payload.message}\n` : JSON.stringify({ ok: false, error: payload }) + '\n');
    return 1;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = await main();
}
