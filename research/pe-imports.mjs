// Read the shipped PE import directory without loading any of its libraries.
import { readFileSync } from 'node:fs';
const path=process.argv[2]??new URL('../native/engine.exe',import.meta.url);
const bytes=readFileSync(path);const pe=bytes.readUInt32LE(0x3c);
if(bytes.subarray(pe,pe+4).toString()!=='PE\0\0')throw new Error('INVALID_PE');
const optional=pe+24;const size=bytes.readUInt16LE(pe+20);const count=bytes.readUInt16LE(pe+6);
if(bytes.readUInt16LE(optional)!==0x20b)throw new Error('EXPECTED_WINDOWS_X64');
const sections=[];for(let i=0;i<count;i++){const p=optional+size+i*40;sections.push({virtual:bytes.readUInt32LE(p+12),size:Math.max(bytes.readUInt32LE(p+8),bytes.readUInt32LE(p+16)),raw:bytes.readUInt32LE(p+20)});}
const offset=rva=>{const s=sections.find(s=>rva>=s.virtual&&rva<s.virtual+s.size);if(!s)throw new Error('INVALID_RVA');return s.raw+rva-s.virtual;};
const imports=[];let entry=offset(bytes.readUInt32LE(optional+112+8));
for(;bytes.subarray(entry,entry+20).some(b=>b!==0);entry+=20){const start=offset(bytes.readUInt32LE(entry+12));imports.push(bytes.subarray(start,bytes.indexOf(0,start)).toString());}
if(imports.some(name=>/python|frida|kernelutil|vcruntime|msvcp|sqlite|zstd|libcrypto/i.test(name)))throw new Error('UNEXPECTED_RUNTIME_DEPENDENCY');
console.log(JSON.stringify({architecture:'x64',imports:imports.sort(),externalCryptoRuntime:false}));
