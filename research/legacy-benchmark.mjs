import { createRequire } from 'node:module';
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
const [mode,source,legacy,output]=process.argv.slice(2);
if(mode==='--worker'){
  const task=output;const require=createRequire(import.meta.url);const api=require(join(resolve(legacy),'src/database.js'));
  const db=api.openDatabase(resolve(source));let result;const t=performance.now();
  if(task==='sessions')result=api.listSessions(db,{limit:20});
  else if(task==='history50')result=api.history(db,{id:'group:123456',kind:'group',peerUin:'123456',table:'group_123456',name:''},{limit:50});
  else result=api.search(db,'为什么',{limit:50});
  const ms=performance.now()-t;db.close();console.log(JSON.stringify({ms:Math.round(ms),rows:Array.isArray(result)?result.length:result.messages.length,peakMemoryBytes:process.resourceUsage().maxRSS*1024}));
}else{
  const report={dataset:'same 120003-message synthetic source',trials:5,timing:'legacy Node backend only; excludes CLI startup and configuration',queries:{}};
  for(const task of ['sessions','history50','search3']){
    const times=[];let peak=0,rows=0;for(let n=0;n<5;n++){
      const p=spawnSync(process.execPath,[new URL(import.meta.url).pathname.replace(/^\/(\w:)/,'$1'),'--worker',source,legacy,task],{windowsHide:true,encoding:'utf8',timeout:120000,env:{...process.env,NODE_NO_WARNINGS:'1'}});
      if(p.status!==0)throw new Error(p.stderr);const r=JSON.parse(p.stdout);times.push(r.ms);rows=r.rows;peak=Math.max(peak,r.peakMemoryBytes);
    }times.sort((a,b)=>a-b);report.queries[task]={medianMs:times[2],maxMs:times[4],rows,peakMemoryBytes:peak};
  }
  if(output)writeFileSync(output,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
}
