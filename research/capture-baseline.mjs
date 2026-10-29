import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
const [qq,wechat,output]=process.argv.slice(2);if(!qq||!wechat||!output)throw new Error('usage: capture-baseline.mjs QQ_ROOT WECHAT_ROOT OUTPUT');
const git=(dir,args)=>{const p=spawnSync('git',['-C',dir,...args],{encoding:'utf8',windowsHide:true,maxBuffer:8*1024*1024});if(p.status!==0)throw new Error(p.stderr);return p.stdout.trim();};
const result={capturedAt:new Date().toISOString(),scope:'current working trees, including uncommitted source and tests',projects:{}};
for(const [platform,dir] of [['qq',qq],['wechat',wechat]]){
  const files=git(dir,['ls-files','-co','--exclude-standard','-z']).split('\0').filter(name=>/^(src\/|test\/|wechat_cli\/|package(?:-lock)?\.json$|pyproject\.toml$)/.test(name)&&!name.endsWith('/'));
  const hashes={};for(const name of files.sort()){try{hashes[name]=createHash('sha256').update(readFileSync(join(dir,name))).digest('hex');}catch(error){if(error.code!=='EISDIR')throw error;}}
  result.projects[platform]={commit:git(dir,['rev-parse','HEAD']),changedSourceFiles:git(dir,['diff','--name-only']).split('\n').filter(Boolean),sourceHashes:hashes};
}
mkdirSync(resolve(output,'..'),{recursive:true});writeFileSync(output,JSON.stringify(result,null,2)+'\n');
console.log(JSON.stringify(Object.fromEntries(Object.entries(result.projects).map(([key,v])=>[key,{commit:v.commit,changedSourceFiles:v.changedSourceFiles,hashedFiles:Object.keys(v.sourceHashes).length}]))));
