// Validate streaming memory and stable export identifiers without logging records.
import { DatabaseSync } from 'node:sqlite';
import { spawnSync } from 'node:child_process';
import { createReadStream } from 'node:fs';
import { stat, unlink, writeFile } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
const root=fileURLToPath(new URL('../',import.meta.url));
const [platform,state,reportPath]=process.argv.slice(2);const home=resolve(state);
const db=new DatabaseSync(join(home,'index.sqlite'),{readOnly:true});
const profile=JSON.parse(db.prepare('SELECT body FROM profiles WHERE platform=?').get(platform).body);
const chat=db.prepare('SELECT conversation_id,count(*) AS n FROM messages WHERE source_id=? GROUP BY conversation_id ORDER BY n DESC LIMIT 1').get(profile.sourceId).conversation_id;
const expected=db.prepare('SELECT id FROM messages WHERE source_id=? AND conversation_id=? ORDER BY time,sequence,id LIMIT ?');
const results=[];
for(const limit of [1000,100000,250000]){
  const output=join(root,`.local/export-${platform}-${limit}.jsonl`);
  const started=performance.now();
  const p=spawnSync(process.execPath,[join(root,'dist/cli.js'),platform,'export',chat,'--account',profile.account,'--data-dir',home,'--format','jsonl','--limit',String(limit),'--output',output,'--force'],{windowsHide:true,encoding:'utf8',timeout:180000,maxBuffer:1024*1024});
  assert.equal(p.status,0,p.stderr);const data=JSON.parse(p.stdout).data;
  const ids=expected.all(profile.sourceId,chat,limit).map(r=>r.id);let count=0,header=true;
  for await(const line of createInterface({input:createReadStream(output),crlfDelay:Infinity})){
    if(header){assert.equal(JSON.parse(line)._type,'header');header=false;continue;}
    assert.equal(JSON.parse(line).id,ids[count++]);
  }
  assert.equal(count,ids.length);assert.equal(data.count,count);
  results.push({limit,count,wallMs:Math.round(performance.now()-started),bytes:(await stat(output)).size,enginePeakMemoryBytes:data.peakMemoryBytes,nodePeakMemoryBytes:data.nodePeakMemoryBytes});
  await unlink(output);
}
db.close();const result={platform,exports:results};if(reportPath)await writeFile(reportPath,JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result));
