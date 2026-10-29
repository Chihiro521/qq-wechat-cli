import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const root=fileURLToPath(new URL('../',import.meta.url));
const archive=resolve(process.argv[2]??join(root,'artifacts/qq-wechat-cli-0.1.0.tgz'));
assert.ok(existsSync(archive),'需要真实 npm 压缩包');
const npm=process.env.npm_execpath??join(dirname(process.execPath),'node_modules/npm/bin/npm-cli.js');
assert.ok(existsSync(npm),'找不到当前 Node.js 附带的 npm CLI');
const base=process.env.QQ_WECHAT_SMOKE_ROOT??tmpdir();mkdirSync(base,{recursive:true});
const sandbox=mkdtempSync(join(base,'qq-wechat-package-'));
const install=join(sandbox,'install');
try{
  const setup=spawnSync(process.execPath,[npm,'install','--prefix',install,'--cache',join(sandbox,'npm-cache'),'--ignore-scripts','--no-audit','--no-fund','--package-lock=false',archive],{encoding:'utf8',windowsHide:true,timeout:120000});
  assert.equal(setup.status,0,setup.stderr||setup.stdout);
  const pkg=join(install,'node_modules/qq-wechat-cli');
  const files=[];const walk=(dir)=>{for(const name of readdirSync(dir,{withFileTypes:true})){const path=join(dir,name.name);if(name.isDirectory())walk(path);else files.push(path.slice(pkg.length+1).replaceAll('\\','/'));}};walk(pkg);
  assert.ok(files.every(name=>/^(package\.json|README\.md|NOTICE|LICENSE|LICENSE-APACHE-2\.0|THIRD-PARTY-NOTICES|docs\/[a-z]+\.md|research\/QQ-CODEC\.md|dist\/[a-z]+\.js|native\/engine\.exe)$/.test(name)),files.join('\n'));
  for(const file of ['dist/cli.js','dist/engine.js','dist/export.js','native/engine.exe','LICENSE','LICENSE-APACHE-2.0','THIRD-PARTY-NOTICES','docs/validation.md']) assert.ok(files.includes(file),`安装包缺少 ${file}`);
  assert.equal(readFileSync(join(pkg,'native/engine.exe')).subarray(0,2).toString(),'MZ');
  const env={...process.env,PATH:[dirname(process.execPath),join(process.env.SystemRoot??'C:\\Windows','System32')].join(';'),NODE_NO_WARNINGS:'1'};
  delete env.QQ_WECHAT_ENGINE;delete env.QQ_WECHAT_HOME;
  for(const program of ['python','cargo','rustc']){
    assert.notEqual(spawnSync(program,['--version'],{env,windowsHide:true,encoding:'utf8'}).status,0,`${program} 仍在验证 PATH 中`);
  }
  const cli=join(pkg,'dist/cli.js');
  const call=(platform,command,args=[],home=join(sandbox,'state'))=>{
    const p=spawnSync(process.execPath,[cli,platform,command,...args,'--data-dir',home],{env,windowsHide:true,encoding:'utf8',timeout:60000,maxBuffer:8*1024*1024});
    assert.equal(p.status,0,p.stderr||p.stdout);return JSON.parse(p.stdout);
  };
  for(const platform of ['qq','wechat']){
    const fixture=JSON.parse(readFileSync(join(root,'test/fixtures',`${platform}.json`),'utf8'));
    const source=join(sandbox,platform);mkdirSync(source,{recursive:true});const key=join(source,'keys.json');
    let initArgs;
    if(platform==='qq'){
      const db=join(source,'Msg3.0.db');writeFileSync(db,Buffer.from(fixture.encrypted,'hex'));writeFileSync(key,JSON.stringify({qq:'42'.repeat(16)}));
      initArgs=['--db',db,'--keys-file',key];
    }else{
      const keys={};for(const f of fixture.files){const path=join(source,f.path);mkdirSync(dirname(path),{recursive:true});writeFileSync(path,Buffer.from(f.encrypted,'hex'));keys[f.path]={enc_key:'24'.repeat(32),salt:f.salt};}
      writeFileSync(key,JSON.stringify(keys));initArgs=['--db-dir',source,'--keys-file',key];
    }
    call(platform,'init',initArgs);assert.equal(call(platform,'refresh').data.changed,false);
    assert.equal(call(platform,'status').data.initialized,true);
    const chat=platform==='qq'?'group:123456':fixture.chat;
    assert.equal(call(platform,'history',[chat]).data.count,3);
    assert.ok(call(platform,'search',['中文搜索']).data.count>0);
    for(const command of ['sessions','contacts','favorites','unread','new-messages'])call(platform,command);
    for(const command of ['stats','members','summarize'])call(platform,command,[chat]);
    const output=join(sandbox,`${platform}.jsonl`);call(platform,'export',[chat,'--format',platform==='qq'?'chatlab':'jsonl','--output',output]);
    assert.ok(readFileSync(output,'utf8').includes(platform==='qq'?'platformMessageId':'originalId'));
    call(platform,'clean');call(platform,'refresh');call(platform,'clean',['--all']);assert.equal(call(platform,'status').data.initialized,false);
  }
  // Exercise the installed npm Windows launcher as well as its Node entry point.
  const launcher=join(install,'node_modules/.bin/qq-wechat-cli.cmd');
  const launched=spawnSync(process.env.ComSpec??'C:\\Windows\\System32\\cmd.exe',['/d','/s','/c',`""${launcher}" --version"`],{env,windowsHide:true,windowsVerbatimArguments:true,encoding:'utf8'});
  assert.equal(launched.status,0,launched.stderr);assert.equal(launched.stdout.trim(),'0.1.0');
  console.log(JSON.stringify({installed:true,packageFiles:files.length,runtime:'Node.js only',encryptedQQ:true,encryptedWeChat:true,commands:true,launcher:true}));
}finally{
  // Only the exact directory returned by mkdtemp under the chosen test root is removed.
  assert.equal(dirname(resolve(sandbox)),resolve(base));
  rmSync(sandbox,{recursive:true,force:true,maxRetries:3});
}
