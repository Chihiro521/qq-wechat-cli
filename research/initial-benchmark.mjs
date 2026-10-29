// Measure first decryption and first indexing separately in fresh state directories.
import { DatabaseSync } from 'node:sqlite';
import { spawnSync } from 'node:child_process';
import { existsSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../',import.meta.url));
const [platform,existing,fresh,reportPath]=process.argv.slice(2);const home=resolve(fresh);
if(existsSync(join(home,'index.sqlite')))throw new Error('需要全新的首次同步状态目录');
const db=new DatabaseSync(join(existing,'index.sqlite'),{readOnly:true});const p=JSON.parse(db.prepare('SELECT body FROM profiles WHERE platform=?').get(platform).body);db.close();
const started=performance.now();const request=spawnSync(process.execPath,[join(root,'dist/cli.js'),platform,platform==='qq'?'init':'import',platform==='qq'?'--db':'--db-dir',p.source,'--account','initial-validation','--data-dir',home],{windowsHide:true,encoding:'utf8',timeout:600000,maxBuffer:1024*1024});
if(request.status!==0)throw new Error(request.stderr);const body=JSON.parse(request.stdout);
const result={platform,wallMs:Math.round(performance.now()-started),timing:body.data.timing,enginePeakMemoryBytes:body.meta.peakMemoryBytes};
if(reportPath)writeFileSync(reportPath,JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result));
