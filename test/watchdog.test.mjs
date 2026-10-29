import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { enginePath } from '../dist/engine.js';

async function until(check, timeout=4000){const start=Date.now();while(!check()){assert.ok(Date.now()-start<timeout,'等待子进程超时');await delay(10);}}
test('pause watchdog resumes on timeout and worker pipe closure; checks process identity',async()=>{
  const ticker=spawn(process.execPath,['-e',"setInterval(()=>process.stdout.write('.'),15)"],{windowsHide:true,stdio:['ignore','pipe','pipe']});
  let ticks=0;ticker.stdout.on('data',data=>{ticks+=data.length;});
  try{
    await until(()=>ticks>2);
    const processInfo=spawnSync('pwsh',['-NoProfile','-Command',`(Get-Process -Id ${ticker.pid}).StartTime.ToUniversalTime().ToFileTimeUtc()`],{encoding:'utf8',windowsHide:true});
    assert.equal(processInfo.status,0,processInfo.stderr);
    const identity=processInfo.stdout.trim();assert.match(identity,/^\d+$/);
    const watchdog=(ms,id=identity)=>spawn(enginePath(),['--pause-watchdog',String(ticker.pid),id,String(ms)],{windowsHide:true,stdio:['pipe','pipe','pipe']});
    for(const mode of ['timeout','closed-pipe']){
      const guard=watchdog(mode==='timeout'?250:5000);
      const done=once(guard,'close');let ready='';guard.stdout.on('data',data=>{ready+=data;});
      await until(()=>ready.includes('PAUSED'));
      await delay(20);const paused=ticks;await delay(50);assert.equal(ticks,paused);
      if(mode==='closed-pipe')guard.stdin.destroy();
      const start=Date.now();assert.equal((await done)[0],0);assert.ok(Date.now()-start<1500);
      await until(()=>ticks>paused);
    }
    const wrong=watchdog(250,String(BigInt(identity)+1n));const wrongDone=once(wrong,'close');wrong.stdin.end();
    assert.notEqual((await wrongDone)[0],0);
    const before=ticks;await until(()=>ticks>before);
  }finally{ticker.kill();await once(ticker,'close');}
});
