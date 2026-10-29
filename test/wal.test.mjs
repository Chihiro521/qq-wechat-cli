import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { createCipheriv, createHash, createHmac, pbkdf2Sync } from 'node:crypto';
import { workspace, wxFixture, run, fail } from './helpers.mjs';

function checksum(bytes, state = [0, 0]) {
  let [a, b] = state;
  for (let i = 0; i < bytes.length; i += 8) {
    a = (a + bytes.readUInt32LE(i) + b) >>> 0;
    b = (b + bytes.readUInt32LE(i + 4) + a) >>> 0;
  }
  return [a, b];
}
function pageEncrypt(page, number, salt) {
  const key = Buffer.alloc(32, 0x24); const iv = createHash('md5').update(page).digest();
  const cipher = createCipheriv('aes-256-cbc', key, iv); cipher.setAutoPadding(false);
  const body = Buffer.concat([cipher.update(page.subarray(number === 1 ? 16 : 0, 4016)), cipher.final()]);
  const macSalt = Buffer.from(salt.map(v => v ^ 0x3a));
  const macKey = pbkdf2Sync(key, macSalt, 2, 32, 'sha512'); const pg = Buffer.alloc(4); pg.writeUInt32LE(number);
  const mac = createHmac('sha512', macKey).update(body).update(iv).update(pg).digest();
  return Buffer.concat([number === 1 ? salt : Buffer.alloc(0), body, iv, mac]);
}
function wal(previous, before, after, salt, commit = true, generation = 1) {
  let header = previous?.subarray(0, 32);
  if (!header) {
    header = Buffer.alloc(32); header.writeUInt32BE(0x377f0682, 0); header.writeUInt32BE(3007000, 4); header.writeUInt32BE(4096, 8);
    header.writeUInt32BE(generation, 12); header.writeUInt32BE(0x12345678 + generation, 16); header.writeUInt32BE(0x87654321, 20);
    const state = checksum(header.subarray(0, 24)); header.writeUInt32BE(state[0], 24); header.writeUInt32BE(state[1], 28);
  }
  let chain = previous && previous.length > 32 ? [previous.readUInt32BE(previous.length - 4096 - 8), previous.readUInt32BE(previous.length - 4096 - 4)] : [header.readUInt32BE(24), header.readUInt32BE(28)];
  const changes = [];
  for (let number = 1; number <= after.length / 4096; number++) {
    const page = after.subarray((number - 1) * 4096, number * 4096);
    if (!page.equals(before.subarray((number - 1) * 4096, number * 4096))) changes.push([number, page]);
  }
  const frames = changes.map(([number, page], i) => {
    const hdr = Buffer.alloc(24); hdr.writeUInt32BE(number); hdr.writeUInt32BE(commit && i === changes.length - 1 ? after.length / 4096 : 0, 4); header.copy(hdr, 8, 16, 24);
    const encrypted = pageEncrypt(page, number, salt); chain = checksum(hdr.subarray(0, 8), chain); chain = checksum(encrypted, chain); hdr.writeUInt32BE(chain[0], 16); hdr.writeUInt32BE(chain[1], 20);
    return Buffer.concat([hdr, encrypted]);
  });
  return Buffer.concat([previous ?? header, ...frames]);
}

test('committed encrypted WAL updates match full rebuild; tails and resets are handled', () => {
  const w = workspace(); try {
    const f = wxFixture(w.dir); const file = f.fixture.files.find(v => v.path === 'message/message_0.db');
    const table = `Msg_${createHash('md5').update(f.fixture.chat).digest('hex')}`;
    const plain = join(w.dir, 'editable.db'); writeFileSync(plain, Buffer.from(file.plain, 'hex'));
    const mutate = text => { const db = new DatabaseSync(plain); db.prepare(`UPDATE ${table} SET message_content=? WHERE local_id=1`).run(`wxid_a:\n${text}`); db.close(); return readFileSync(plain); };
    const sourceWal = join(f.path, 'message/message_0.db-wal'); const salt = Buffer.from(file.salt, 'hex');
    const base = Buffer.from(file.plain, 'hex'); const first = mutate('第一次更新');
    let log = wal(null, base, first, salt); writeFileSync(sourceWal, log);
    run(w.home, 'wechat', 'init', '--db-dir', f.path, '--keys-file', f.key);
    assert.equal(run(w.home, 'wechat', 'search', '第一次更新').data.count, 1);
    const second = mutate('第二次更新'); log = wal(log, first, second, salt); writeFileSync(sourceWal, log);
    const refreshed = run(w.home, 'wechat', 'refresh').data;
    assert.ok(refreshed.decryptedPages > 0 && refreshed.decryptedPages < base.length / 4096 + 3);
    assert.equal(run(w.home, 'wechat', 'search', '第二次更新').data.count, 1);
    const fullHome = join(w.dir, 'full-state'); run(fullHome, 'wechat', 'init', '--db-dir', f.path, '--keys-file', f.key);
    assert.deepEqual(run(w.home, 'wechat', 'history', f.fixture.chat).data.messages, run(fullHome, 'wechat', 'history', f.fixture.chat).data.messages);
    const tail = mutate('尚未提交'); writeFileSync(sourceWal, wal(log, second, tail, salt, false));
    run(w.home, 'wechat', 'refresh'); assert.equal(run(w.home, 'wechat', 'search', '尚未提交').data.count, 0);
    writeFileSync(sourceWal, wal(null, base, second, salt, true, 2)); run(w.home, 'wechat', 'refresh');
    assert.equal(run(w.home, 'wechat', 'search', '第二次更新').data.count, 1);
    const corrupted = readFileSync(sourceWal); corrupted[24] ^= 1; writeFileSync(sourceWal, corrupted);
    assert.notEqual(fail(w.home, 'wechat', 'refresh').status, 0);
    assert.equal(run(w.home, 'wechat', 'search', '第二次更新').data.count, 1);
    const validReset=wal(null,base,second,salt,true,2);
    writeFileSync(sourceWal,validReset.subarray(0,32));
    assert.notEqual(fail(w.home,'wechat','refresh').status,0);
    assert.equal(run(w.home,'wechat','search','第二次更新').data.count,1);
    // A checkpoint changes the base and removes the old WAL; rebuild that shard.
    mutate('checkpoint更新');
    const plainDb=new DatabaseSync(plain);plainDb.exec(`DELETE FROM ${table} WHERE local_id=2`);plainDb.close();
    const checkpoint=readFileSync(plain);const pages=[];
    for(let n=1;n<=checkpoint.length/4096;n++)pages.push(pageEncrypt(checkpoint.subarray((n-1)*4096,n*4096),n,salt));
    writeFileSync(join(f.path,'message/message_0.db'),Buffer.concat(pages));rmSync(sourceWal);
    run(w.home,'wechat','refresh');
    const checkpointHome=join(w.dir,'checkpoint-state');run(checkpointHome,'wechat','init','--db-dir',f.path,'--keys-file',f.key);
    assert.deepEqual(run(w.home,'wechat','history',f.fixture.chat).data.messages,run(checkpointHome,'wechat','history',f.fixture.chat).data.messages);
  } finally { w.close(); }
});
