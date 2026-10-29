import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { exportMessages } from '../dist/export.js';
import { workspace, qqFixture, run, fail } from './helpers.mjs';

const message = { senderId:'1', senderName:'测试', time:1700000000, text:'内容', originalId:'stable-1' };
test('export validates source paths before creating any output files', () => {
  const w=workspace();try{
    const f=qqFixture(w.dir);run(w.home,'qq','init','--db',f.path);
    const before=readFileSync(f.path);
    assert.notEqual(fail(w.home,'qq','export','group:123456','--format','jsonl','--output',f.path,'--force').status,0);
    assert.deepEqual(readFileSync(f.path),before);
    assert.equal(readdirSync(w.dir).filter(n=>n.endsWith('.tmp')).length,0);
    const ownIndex=join(w.home,'index.sqlite');
    assert.notEqual(fail(w.home,'qq','export','group:123456','--output',ownIndex,'--force').status,0);
    const unrelated=join(w.dir,'export/index.sqlite.jsonl');
    run(w.home,'qq','export','group:123456','--format','jsonl','--output',unrelated);
    assert.equal(readFileSync(unrelated,'utf8').trim().split('\n').length,4);
  }finally{w.close();}
});

test('stream errors and concurrent publication preserve existing output',async()=>{
  const w=workspace();try{
    const output=join(w.dir,'race.jsonl');
    async function* race(){yield {kind:'start',meta:{}};writeFileSync(output,'created concurrently');yield {kind:'row',data:message};yield {kind:'end'};}
    await assert.rejects(exportMessages(race(),{output,format:'jsonl'}),/OUTPUT_EXISTS/);
    assert.equal(readFileSync(output,'utf8'),'created concurrently');
    async function* broken(){yield {kind:'start',meta:{}};yield {kind:'row',data:message};throw new Error('stream interrupted');}
    await assert.rejects(exportMessages(broken(),{output,format:'jsonl',force:true}),/stream interrupted/);
    assert.equal(readFileSync(output,'utf8'),'created concurrently');
    assert.equal(readdirSync(w.dir).filter(n=>n.endsWith('.tmp')).length,0);
    const target=join(w.dir,'missing/out.jsonl');
    async function* rejected(){throw new Error('OUTPUT_IS_SOURCE');}
    await assert.rejects(exportMessages(rejected(),{output:target}),/OUTPUT_IS_SOURCE/);
    assert.equal(existsSync(join(w.dir,'missing')),false);
  }finally{w.close();}
});
