import test from 'node:test';
import assert from 'node:assert/strict';
import { DatabaseSync } from 'node:sqlite';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { writeFileSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { enginePath } from '../dist/engine.js';
import { workspace, qqFixture, wxFixture, run, fail } from './helpers.mjs';

test('wrong keys, truncated files and changing account sources retain valid results',()=>{
  const w=workspace();try{
    const f=qqFixture(w.dir,true);const wrong=join(w.dir,'wrong.json');writeFileSync(wrong,JSON.stringify({qq:'00'.repeat(16)}));
    assert.match(fail(w.home,'qq','init','--db',f.path,'--keys-file',wrong).stderr,/KEY_MISMATCH/);
    run(w.home,'qq','init','--db',f.path,'--keys-file',f.key);
    const before=readFileSync(f.path);writeFileSync(f.path,before.subarray(0,before.length-1));
    assert.notEqual(fail(w.home,'qq','refresh').status,0);
    assert.equal(run(w.home,'qq','history','group:123456').data.count,3);
    writeFileSync(f.path,before);
    const next=join(w.dir,'replacement');awaitlessMkdir(next);
    const g=qqFixture(next);const db=new DatabaseSync(g.path);db.exec('DELETE FROM group_123456 WHERE Rand<>3');db.close();
    run(w.home,'qq','refresh','--db',g.path);
    assert.equal(run(w.home,'qq','history','group:123456').data.count,1);
  }finally{w.close();}
});
import { mkdirSync as awaitlessMkdir } from 'node:fs';

test('removed WeChat shards remove their messages and auxiliary records',()=>{
  const w=workspace();try{
    const f=wxFixture(w.dir,false);run(w.home,'wechat','init','--db-dir',f.path);
    rmSync(join(f.path,'message/message_0.db'));rmSync(join(f.path,'contact/contact.db'));
    run(w.home,'wechat','refresh');
    assert.equal(run(w.home,'wechat','status').data.accounts[0].messages,0);
    assert.equal(run(w.home,'wechat','contacts').data.count,0);
    assert.equal(run(w.home,'wechat','members',f.fixture.chat).data.count,0);
  }finally{w.close();}
});

test('concurrent refresh rejects; cancellation rolls back the index and next refresh recovers',async()=>{
  const w=workspace();let child;
  try{
    const f=qqFixture(w.dir);run(w.home,'qq','init','--db',f.path);
    const db=new DatabaseSync(f.path);const row=db.prepare('SELECT * FROM group_123456 LIMIT 1').get();
    db.exec('BEGIN');const insert=db.prepare('INSERT INTO group_123456 VALUES(?,?,?,?,?)');
    for(let i=0;i<60000;i++)insert.run(1700000100+i,i+100,10001,row.MsgContent,row.Info);
    db.exec('COMMIT');db.close();
    child=spawn(enginePath(),[],{windowsHide:true,stdio:['pipe','pipe','pipe']});
    const closed=once(child,'close');let progress='';child.stderr.on('data',data=>{progress+=data;});
    child.stdin.end(JSON.stringify({protocol:1,platform:'qq',command:'refresh',dataDir:w.home,args:{}})+'\n');
    const start=Date.now();while(!progress.includes('已索引 50000')){assert.ok(Date.now()-start<60000,progress);assert.equal(child.exitCode,null);await delay(10);}
    const concurrent=fail(w.home,'qq','refresh');assert.notEqual(concurrent.status,0);assert.match(concurrent.stderr,/REFRESH_BUSY/);
    assert.equal(run(w.home,'qq','history','group:123456').data.count,3);
    child.kill();await closed;child=undefined;
    assert.equal(run(w.home,'qq','history','group:123456').data.count,3);
    run(w.home,'qq','refresh');assert.equal(run(w.home,'qq','stats','group:123456').data.totalMessages,60003);
  }finally{if(child){child.kill();await once(child,'close');}w.close();}
});
