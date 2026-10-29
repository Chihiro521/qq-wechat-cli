// Compare old JavaScript TLV decoding with the Rust model, without printing chats.
import { readFileSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { createRequire } from 'node:module';
import { resolve, join } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
const [state, rows, legacy] = process.argv.slice(2);
if (!state || !rows || !legacy) throw new Error('usage: compare-qq.mjs STATE ROWS LEGACY_ROOT');
const require = createRequire(import.meta.url);
const { messageFromRow } = require(join(resolve(legacy), 'src/database.js'));
const db = new DatabaseSync(join(state, 'index.sqlite'), { readOnly:true });
const profile = JSON.parse(db.prepare("SELECT body FROM profiles WHERE platform='qq'").get().body);
const query = db.prepare('SELECT body FROM messages WHERE id=?');
const prefix = `qq:${profile.account.replaceAll('%','%25').replaceAll(':','%3A')}:`;
const result = { sampled:0, missing:0, identityMismatches:0, textMismatches:0, typeMismatches:0, senderMismatches:0, elementMismatches:0 };
for (const row of JSON.parse(readFileSync(rows,'utf8'))) {
  row.MsgContent = Buffer.from(row.MsgContent,'hex');
  const chat = { kind:row.kind, peerUin:row.peer, id:row.kind==='group'?`group:${row.peer}`:`${row.kind}:${row.peer}`, name:'' };
  const old = messageFromRow(chat,row);
  const found = query.get(prefix+old.id);result.sampled++;
  if(!found){result.missing++;continue;}
  const current = JSON.parse(found.body);
  result.identityMismatches += +(current.originalId!==old.id || current.time!==old.time || current.metadata.rand!==old.rand);
  result.textMismatches += +(current.text!==old.text);
  result.typeMismatches += +(current.type!==old.type);
  result.senderMismatches += +(current.senderId!==old.senderUin || current.senderName!==old.senderName);
  result.elementMismatches += +!isDeepStrictEqual(current.elements,old.elements);
}
db.close();console.log(JSON.stringify(result));
