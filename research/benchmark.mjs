// Full CLI wall-clock timings. Never prints source names, IDs or message contents.
import { spawnSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { DatabaseSync } from 'node:sqlite';
import { join, resolve } from 'node:path';
import { statSync, readdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../',import.meta.url));
const [platform,state,output]=process.argv.slice(2);if(!['qq','wechat'].includes(platform)||!state)throw new Error('usage: benchmark.mjs PLATFORM STATE [REPORT]');
const home=resolve(state);const db=new DatabaseSync(join(home,'index.sqlite'),{readOnly:true});
const profile=JSON.parse(db.prepare('SELECT body FROM profiles WHERE platform=?').get(platform).body);
const summary=db.prepare('SELECT count(*) AS count,min(time) AS first,max(time) AS last FROM messages WHERE source_id=?').get(profile.sourceId);
const chat=db.prepare('SELECT conversation_id,count(*) AS n FROM messages WHERE source_id=? GROUP BY conversation_id ORDER BY n DESC LIMIT 1').get(profile.sourceId).conversation_id;
const disk=path=>statSync(path).isDirectory()?readdirSync(path).reduce((n,v)=>n+disk(join(path,v)),0):statSync(path).size;
db.close();
const invoke=(command,args=[])=>{
  const started=performance.now();const p=spawnSync(process.execPath,[join(root,'dist/cli.js'),platform,command,...args,'--account',profile.account,'--data-dir',home],{windowsHide:true,encoding:'utf8',timeout:120000,maxBuffer:16*1024*1024,env:{...process.env,NODE_NO_WARNINGS:'1'}});
  if(p.status!==0)throw new Error(p.stderr);return {ms:performance.now()-started,body:JSON.parse(p.stdout)};
};
const report={platform,messages:summary.count,timeRange:[summary.first,summary.last],stateBytes:disk(home),indexBytes:statSync(join(home,'index.sqlite')).size,trials:30,queries:{}};
for(const [label,command,args]of [['sessions','sessions',[]],['history50','history',[chat,'--limit','50']],['search3','search',['为什么','--limit','50']]]){
  invoke(command,args);const times=[];let count=0,peak=0;for(let n=0;n<30;n++){const r=invoke(command,args);times.push(r.ms);count=r.body.data.count;peak=Math.max(peak,r.body.meta.peakMemoryBytes??0);}
  times.sort((a,b)=>a-b);report.queries[label]={p50Ms:Math.round(times[14]),p95Ms:Math.round(times[28]),returned:count,enginePeakMemoryBytes:peak};
}
if(output)writeFileSync(output,JSON.stringify(report,null,2)+'\n');
const refresh=invoke('refresh');report.refresh={wallMs:Math.round(refresh.ms),...refresh.body.data,enginePeakMemoryBytes:refresh.body.meta.peakMemoryBytes??0};
if(output)writeFileSync(output,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
