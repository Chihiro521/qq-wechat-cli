import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { spawnSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { workspace, run, fail, qqFixture, wxFixture, cli, root } from './helpers.mjs';

test('help and version work before initialization', () => {
  for (const arg of ['--help', '--version']) {
    const result = spawnSync(process.execPath, [cli, arg], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr); assert.match(result.stdout, /0\.1\.0/);
  }
});

for (const encrypted of [false, true]) test(`QQ complete query flow, encrypted=${encrypted}`, () => {
  const w = workspace(); try {
    const f = qqFixture(w.dir, encrypted);
    const before = readFileSync(f.path);
    const initialized = run(w.home, 'qq', 'init', '--db', f.path, '--account', '10000', '--keys-file', f.key);
    assert.equal(initialized.ok, true);
    const sessions = run(w.home, 'qq', 'sessions').data.sessions;
    assert.equal(sessions.length, 2); assert.ok(sessions.some(s => s.name === '测试群'));
    const history = run(w.home, 'qq', 'history', 'group:123456').data;
    assert.equal(history.messages.length, 3); assert.equal(history.messages[0].text, '第三条 world');
    assert.equal(run(w.home, 'qq', 'search', '中文搜索').data.count, 2);
    assert.equal(run(w.home, 'qq', 'search', '中文').data.count, 2);
    assert.equal(run(w.home, 'qq', 'search', 'hello', '--chat', 'group:123456').data.count, 1);
    assert.equal(run(w.home, 'qq', 'stats', 'group:123456').data.totalMessages, 3);
    assert.equal(run(w.home, 'qq', 'contacts').data.contacts.length, 1);
    assert.equal(run(w.home, 'qq', 'members', 'group:123456').data.members.length, 3);
    assert.equal(run(w.home, 'qq', 'summarize', 'group:123456', '--chunk-size', '2').data.chunks.length, 2);
    assert.equal(run(w.home, 'qq', 'new-messages').data.baselineEstablished, true);
    assert.equal(run(w.home, 'qq', 'new-messages').data.count, 0);
    const page = run(w.home, 'qq', 'history', 'group:123456', '--limit', '1').data;
    assert.equal(run(w.home, 'qq', 'history', 'group:123456', '--before', page.nextCursor).data.count, 2);
    const refreshed = run(w.home, 'qq', 'refresh').data;
    assert.equal(refreshed.changed, false); assert.equal(refreshed.copiedBytes, 0); assert.equal(refreshed.decryptedPages, 0);
    const out = join(w.dir, 'out/chatlab.jsonl');
    run(w.home, 'qq', 'export', 'group:123456', '--format', 'chatlab', '--output', out);
    const records = readFileSync(out, 'utf8').trim().split('\n').map(JSON.parse);
    assert.equal(records[0].chatlab.version, '0.0.2'); assert.equal(records.filter(v => v._type === 'message').length, 3);
    assert.equal(records.find(v => v._type === 'message').platformMessageId, '123456:1700000000:1');
    assert.notEqual(fail(w.home, 'qq', 'export', 'group:123456', '--output', out).status, 0);
    assert.deepEqual(readFileSync(f.path), before);
    assert.equal(run(w.home, 'qq', 'clean').data.cleaned, true);
    assert.equal(run(w.home, 'qq', 'refresh').data.changed, true);
    run(w.home, 'qq', 'clean', '--all');
    assert.equal(run(w.home, 'qq', 'status').data.initialized, false);
  } finally { w.close(); }
});

for (const encrypted of [false, true]) test(`WeChat complete query flow, encrypted=${encrypted}`, () => {
  const w = workspace(); try {
    const f = wxFixture(w.dir, encrypted);
    run(w.home, 'wechat', 'init', '--db-dir', f.path, '--account', 'wxid_fixture', '--keys-file', f.key);
    assert.equal(run(w.home, 'wechat', 'sessions').data.sessions.length, 1);
    assert.equal(run(w.home, 'wechat', 'unread').data.sessions[0].unread, 2);
    assert.equal(run(w.home, 'wechat', 'history', '测试群').data.count, 3);
    assert.equal(run(w.home, 'wechat', 'search', '中文搜索').data.count, 1);
    assert.equal(run(w.home, 'wechat', 'history', f.fixture.chat, '--type', 'file').data.count, 1);
    assert.equal(run(w.home, 'wechat', 'contacts').data.contacts.length, 3);
    assert.equal(run(w.home, 'wechat', 'contacts', '--detail', 'wxid_a').data.name, 'Alice');
    assert.equal(run(w.home, 'wechat', 'members', f.fixture.chat).data.members.length, 2);
    assert.equal(run(w.home, 'wechat', 'favorites', '--query', '测试文章').data.count, 1);
    assert.equal(run(w.home, 'wechat', 'stats', f.fixture.chat).data.totalMessages, 3);
    assert.equal(run(w.home, 'wechat', 'refresh').data.changed, false);
    const out = join(w.dir, 'out/history.jsonl'); run(w.home, 'wechat', 'export', f.fixture.chat, '--format', 'jsonl', '--output', out);
    assert.equal(readFileSync(out, 'utf8').trim().split('\n').length, 4);
    assert.equal(run(w.home, 'wechat', 'new-messages').data.baselineEstablished, true);
    assert.equal(run(w.home, 'wechat', 'new-messages').data.count, 0);
  } finally { w.close(); }
});

test('platform and account namespaces isolate equal IDs', () => {
  const w = workspace(); try {
    const q = qqFixture(w.dir); const x = wxFixture(w.dir);
    for (const account of ['a', 'b']) run(w.home, 'qq', 'init', '--db', q.path, '--account', account);
    run(w.home, 'wechat', 'init', '--db-dir', x.path, '--keys-file', x.key, '--account', 'a');
    assert.notEqual(fail(w.home, 'qq', 'history', 'group:123456').status, 0);
    const a = run(w.home, 'qq', 'history', 'group:123456', '--account', 'a').data.messages;
    const b = run(w.home, 'qq', 'history', 'group:123456', '--account', 'b').data.messages;
    assert.notEqual(a[0].id, b[0].id);
    run(w.home, 'qq', 'clean', '--all', '--account', 'a');
    assert.equal(run(w.home, 'wechat', 'history', x.fixture.chat).data.count, 3);
    assert.equal(run(w.home, 'qq', 'history', 'group:123456').data.count, 3);
  } finally { w.close(); }
});

test('failed refresh preserves the last successful index', () => {
  const w = workspace(); try {
    const f = wxFixture(w.dir);
    run(w.home, 'wechat', 'init', '--db-dir', f.path, '--keys-file', f.key);
    const path = join(f.path, 'message/message_0.db'); const bytes = readFileSync(path); bytes[4096 + 100] ^= 1; writeFileSync(path, bytes);
    assert.notEqual(fail(w.home, 'wechat', 'refresh').status, 0);
    assert.equal(run(w.home, 'wechat', 'history', f.fixture.chat).data.count, 3);
  } finally { w.close(); }
});

test('plaintext changes, deletions and checkpoints update correctly', () => {
  const w = workspace(); try {
    const f = qqFixture(w.dir);
    run(w.home, 'qq', 'init', '--db', f.path);
    run(w.home, 'qq', 'new-messages');
    const db = new DatabaseSync(f.path);
    const row = db.prepare('SELECT * FROM group_123456 ORDER BY Time DESC LIMIT 1').get();
    const blob = Buffer.from(row.MsgContent); blob.writeUInt32LE(1700000005, 8); blob.writeUInt32LE(5, 12);
    db.prepare('INSERT INTO group_123456 VALUES(?,?,?,?,?)').run(1700000005, 5, 10001, blob, row.Info);
    db.prepare('DELETE FROM group_123456 WHERE Rand=1').run(); db.close();
    assert.equal(run(w.home, 'qq', 'history', 'group:123456').data.count, 3);
    run(w.home, 'qq', 'refresh');
    const fresh = run(w.home, 'qq', 'new-messages').data;
    assert.equal(fresh.count, 1); assert.equal(fresh.messages[0].time, 1700000005);
    assert.equal(run(w.home, 'qq', 'new-messages').data.count, 0);
    assert.equal(run(w.home, 'qq', 'search', '第一条', '--chat', 'group:123456').data.count, 0);
  } finally { w.close(); }
});

test('explicit legacy configuration import leaves the old files intact', () => {
  const w = workspace(); try {
    const f = wxFixture(w.dir); const config = join(w.dir, 'legacy.json');
    const original = JSON.stringify({ db_dir: f.path, keys_file: f.key, decrypted_dir: join(w.dir, 'legacy-cache') }); writeFileSync(config, original);
    run(w.home, 'wechat', 'import', '--config', config);
    assert.equal(run(w.home, 'wechat', 'search', 'hello').data.count, 1);
    assert.equal(readFileSync(config, 'utf8'), original);
  } finally { w.close(); }
});

test('QQ favorites accept textual and nullable timestamp fields', () => {
  const w=workspace();try{
    const f=qqFixture(w.dir);const dir=join(w.dir,'MyCollection');mkdirSync(dir);
    const db=new DatabaseSync(join(dir,'mc3.db'));
    db.exec('CREATE TABLE Cid2DataTable(lid INTEGER,cid TEXT,type TEXT,localTime TEXT,serverTime TEXT,createTime TEXT,collectTime TEXT,data BLOB)');
    db.prepare('INSERT INTO Cid2DataTable VALUES(?,?,?,?,?,?,?,?)').run(1,'cid-1','3',null,'1700000000','1700000000','1700000000',Buffer.from('<card><title>测试收藏</title><url>https://example.com</url></card>'));
    db.close();run(w.home,'qq','init','--db',f.path);
    const result=run(w.home,'qq','favorites').data.favorites;
    assert.equal(result.length,1);assert.equal(result[0].type,'article');assert.equal(result[0].time,1700000000);assert.match(result[0].summary,/测试收藏/);assert.equal(result[0].cid,'cid-1');
  }finally{w.close();}
});

test('relative state directories preserve reusable caches', () => {
  const w=workspace();try{const f=qqFixture(w.dir);const home=relative(root,w.home);run(home,'qq','init','--db',f.path);assert.equal(run(home,'qq','refresh').data.changed,false);}finally{w.close();}
});

test('QQ unsigned timestamps and limited new-message checkpoints retain all records',()=>{
  const w=workspace();try{const f=qqFixture(w.dir);run(w.home,'qq','init','--db',f.path);run(w.home,'qq','new-messages');
    const db=new DatabaseSync(f.path);const row=db.prepare('SELECT * FROM group_123456 LIMIT 1').get();
    const insert=db.prepare('INSERT INTO group_123456 VALUES(?,?,?,?,?)');
    for(const [time,rand]of [[1700001000,41],[-2147483648,-1]]){const blob=Buffer.from(row.MsgContent);blob.writeUInt32LE(time>>>0,8);blob.writeUInt32LE(rand>>>0,12);insert.run(time,rand,10001,blob,row.Info);}db.close();run(w.home,'qq','refresh');
    assert.equal(run(w.home,'qq','unread').data.sessions[0].unread,2);
    assert.equal(run(w.home,'qq','new-messages','--limit','1').data.messages[0].time,1700001000);
    const second=run(w.home,'qq','new-messages','--limit','1').data.messages[0];assert.equal(second.time,2147483648);assert.equal(second.metadata.rand,4294967295);
    assert.equal(run(w.home,'qq','new-messages').data.count,0);
  }finally{w.close();}
});

test('punctuation in account aliases cannot collide with conversation IDs', () => {
  const w=workspace();try{const f=qqFixture(w.dir);const db=new DatabaseSync(f.path);db.exec('CREATE TABLE buddy_123456 AS SELECT * FROM group_123456');db.close();
    for(const account of ['a','a:buddy'])run(w.home,'qq','init','--db',f.path,'--account',account);
    assert.equal(run(w.home,'qq','history','buddy:123456','--account','a').data.count,3);
    assert.equal(run(w.home,'qq','history','group:123456','--account','a:buddy').data.count,3);
  }finally{w.close();}
});

test('QQ display names follow unsigned message time even after older history is inserted',()=>{
  const w=workspace();try{const f=qqFixture(w.dir);const db=new DatabaseSync(f.path);const row=db.prepare('SELECT * FROM group_123456 LIMIT 1').get();
    const info=name=>{const field=Buffer.from('strRecieverShowName','utf16le');const value=Buffer.from(name,'utf16le');const size=Buffer.alloc(4);size.writeUInt32LE(value.length);return Buffer.concat([Buffer.from([8,field.length,0]),Buffer.from(field.map(v=>v^(255-field.length))),size,Buffer.from(value.map(v=>v^(255-value.length)))]);};
    const insert=db.prepare('INSERT INTO group_123456 VALUES(?,?,?,?,?)');insert.run(-2147483648,20,10001,row.MsgContent,info('新的群名'));insert.run(1600000000,21,10001,row.MsgContent,info('旧的群名'));db.close();
    run(w.home,'qq','init','--db',f.path);assert.equal(run(w.home,'qq','sessions').data.sessions.find(s=>s.id==='group:123456').name,'新的群名');
  }finally{w.close();}
});
