import { spawnSync } from 'node:child_process';
import { readFileSync, readdirSync, existsSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../',import.meta.url));
const result=spawnSync('cargo',['metadata','--manifest-path',join(root,'native/Cargo.toml'),'--locked','--filter-platform','x86_64-pc-windows-msvc','--format-version','1'],{encoding:'utf8',windowsHide:true,maxBuffer:8*1024*1024});
if(result.status!==0)throw new Error(result.stderr);
let notice='Third-party source license notices included by the Windows Rust build.\n\n';
for(const pkg of JSON.parse(result.stdout).packages.sort((a,b)=>a.name.localeCompare(b.name))){
  if(pkg.name==='qq-wechat-engine')continue;
  const dir=dirname(pkg.manifest_path);
  const files=readdirSync(dir).filter(name=>/^(LICENSE|COPYING)(\.|-|$)/i.test(name));
  if(pkg.name==='zstd-sys'&&existsSync(join(dir,'zstd/lib/LICENSE')))files.push('zstd/lib/LICENSE');
  notice+=`\n${'='.repeat(72)}\n${pkg.name} ${pkg.version} — ${pkg.license??'see source license'}\n${pkg.repository??''}\n`;
  for(const file of files){try{notice+=`\n--- ${file} ---\n${readFileSync(join(dir,file),'utf8')}\n`;}catch(error){if(error.code!=='EISDIR')throw error;}}
  if(!files.length)notice+='\nLicense expression as declared in the pinned crate manifest above.\n';
}
writeFileSync(join(root,'THIRD-PARTY-NOTICES'),notice);
console.log('已生成 Rust 依赖许可声明');
