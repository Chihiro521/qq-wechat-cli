use crate::{
    model::{Message, Profile, stamp},
    store,
};
use anyhow::{Result, ensure};
use md5::{Digest, Md5};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
        [table],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}
fn bytes(row: &rusqlite::Row, index: usize) -> Vec<u8> {
    use rusqlite::types::ValueRef;
    match row.get_ref(index) {
        Ok(ValueRef::Blob(v)) | Ok(ValueRef::Text(v)) => v.to_vec(),
        _ => vec![],
    }
}
fn content(data: &[u8], compressed: bool) -> Result<String> {
    let raw = if compressed {
        use std::io::Read;
        let decoder = zstd::stream::read::Decoder::new(data)?;
        let mut out = vec![];
        decoder.take(32 * 1024 * 1024 + 1).read_to_end(&mut out)?;
        ensure!(out.len() <= 32 * 1024 * 1024, "CONTENT_TOO_LARGE");
        out
    } else {
        data.to_vec()
    };
    Ok(String::from_utf8_lossy(&raw).into_owned())
}
fn xml_fields(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if text.len() > 1024 * 1024 {
        return out;
    }
    if let Ok(doc) = roxmltree::Document::parse(text) {
        for node in doc.descendants().filter(|n| n.is_element()) {
            if let Some(value) = node.text() {
                if !value.trim().is_empty() {
                    out.entry(node.tag_name().name().to_string())
                        .or_insert(value.trim().to_string());
                }
            }
            for attr in node.attributes() {
                out.entry(format!("{}@{}", node.tag_name().name(), attr.name()))
                    .or_insert(attr.value().to_string());
            }
        }
    }
    out
}
pub fn parse_message(
    raw: &str,
    raw_type: i64,
    is_group: bool,
) -> (String, String, String, Vec<Value>) {
    let (mut sender, mut text) = (String::new(), raw.to_string());
    if is_group {
        if let Some((name, body)) = raw.split_once(":\n") {
            if name.len() < 256 {
                sender = name.into();
                text = body.into();
            }
        }
    }
    let base = raw_type as u64 & 0xffffffff;
    let sub = (raw_type as u64) >> 32;
    let fields = xml_fields(&text);
    let get = |key: &str| fields.get(key).map(String::as_str).unwrap_or("");
    let mut elements = vec![];
    let kind = match base {
        1 => "text",
        3 => "image",
        34 => "voice",
        43 => "video",
        47 => "sticker",
        48 => "location",
        49 => {
            if get("type") == "6" || sub == 6 {
                "file"
            } else {
                "link"
            }
        }
        50 => "call",
        10000 | 10002 => "system",
        42 => "card",
        _ => "unknown",
    }
    .to_string();
    match base {
        3 => text = "[图片]".into(),
        34 => text = format!("[语音] {}", get("voicemsg@voicelength")),
        43 => text = "[视频]".into(),
        47 => text = "[表情]".into(),
        49 => {
            let title = get("title");
            let app_type = get("type").parse::<u64>().unwrap_or(sub);
            text = match app_type {
                57 => {
                    let reference = get("content").chars().take(160).collect::<String>();
                    if reference.is_empty() {
                        title.to_string()
                    } else {
                        format!("{title}\n  ↳ 回复 {}: {reference}", get("displayname"))
                    }
                }
                6 => format!("[文件] {title}"),
                5 => format!("[链接] {title}"),
                33 | 36 | 44 => format!("[小程序] {title}"),
                _ => format!("[链接/文件] {title}"),
            };
        }
        50 => {
            let message = get("msg");
            text = if let Some(duration) = message.strip_prefix("Duration:") {
                format!("[通话] 通话时长 {}", duration.trim())
            } else {
                format!(
                    "[通话] {}",
                    match message {
                        "Canceled" => "已取消",
                        "Line busy" => "对方忙线",
                        "Call not answered" | "Call wasn't answered" => "未接听",
                        s => s,
                    }
                )
            };
        }
        48 => {
            text = format!(
                "[位置] {} {}",
                get("location@label"),
                get("location@poiname")
            )
        }
        10000 => {
            text = if text.is_empty() {
                "[系统]".into()
            } else {
                format!("[系统] {text}")
            }
        }
        10002 => {
            text = if text.is_empty() {
                "[撤回]".into()
            } else {
                format!("[撤回] {text}")
            }
        }
        1 => {}
        _ => text = format!("[{kind}] {text}"),
    }
    if base != 1 {
        let mut element = json!({"type":kind,"rawType":raw_type,"fields":fields});
        if kind == "file" {
            element["filename"] = json!(get("title"));
        }
        if !get("url").is_empty() {
            element["url"] = json!(get("url"));
        }
        elements.push(element);
    } else {
        elements.push(json!({"type":"text","text":text}));
    }
    (sender, text, kind, elements)
}
fn names(conn: &Connection, p: &Profile) -> Result<BTreeMap<String, String>> {
    let mut st = conn.prepare("SELECT id,name FROM contacts WHERE source_id=?")?;
    Ok(st
        .query_map([&p.source_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn import_contacts(conn: &Connection, p: &Profile, path: &Path) -> Result<usize> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure!(
        table_exists(&db, "contact"),
        "DATABASE_SCHEMA_UNSUPPORTED: 缺少微信联系人表"
    );
    conn.execute("DELETE FROM contacts WHERE source_id=?", [&p.source_id])?;
    conn.execute("DELETE FROM members WHERE source_id=?", [&p.source_id])?;
    let columns = columns(&db, "contact")?;
    let field = |name: &str| {
        if columns.contains(&name.to_string()) {
            format!("\"{name}\"")
        } else {
            "NULL".into()
        }
    };
    let fields = [
        "id",
        "username",
        "nick_name",
        "remark",
        "alias",
        "description",
        "small_head_url",
        "big_head_url",
        "verify_flag",
        "local_type",
    ];
    let mut st = db.prepare(&format!(
        "SELECT {} FROM contact",
        fields
            .iter()
            .map(|s| field(s))
            .collect::<Vec<_>>()
            .join(",")
    ))?;
    let mut rows = st.query([])?;
    let mut n = 0;
    while let Some(r) = rows.next()? {
        let id: String = r.get::<_, Option<String>>(1)?.unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let nick = r.get::<_, Option<String>>(2)?.unwrap_or_default();
        let remark = r.get::<_, Option<String>>(3)?.unwrap_or_default();
        let name = if !remark.is_empty() {
            remark.clone()
        } else if !nick.is_empty() {
            nick.clone()
        } else {
            id.clone()
        };
        let body = json!({"id":id,"username":id,"name":name,"nickName":nick,"remark":remark,"alias":r.get::<_,Option<String>>(4)?.unwrap_or_default(),"description":r.get::<_,Option<String>>(5)?.unwrap_or_default(),"avatar":r.get::<_,Option<String>>(6)?.or(r.get::<_,Option<String>>(7)?),"verifyFlag":r.get::<_,Option<i64>>(8)?.unwrap_or(0),"localType":r.get::<_,Option<i64>>(9)?,"isGroup":id.contains("@chatroom"),"coverage":"client_contacts"});
        conn.execute(
            "INSERT INTO contacts VALUES(?,?,?,?)",
            params![p.source_id, id, name, body.to_string()],
        )?;
        n += 1;
    }
    if table_exists(&db, "chatroom_member") {
        let names = names(conn, p)?;
        let mut st=db.prepare("SELECT room.username,member.username,member.nick_name,member.remark FROM chatroom_member cm JOIN contact room ON room.id=cm.room_id JOIN contact member ON member.id=cm.member_id")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let room: String = r.get(0)?;
            let id: String = r.get(1)?;
            let name = names.get(&id).cloned().unwrap_or(id.clone());
            let owner: Option<String> = if table_exists(&db, "chat_room") {
                db.query_row("SELECT cr.owner FROM chat_room cr JOIN contact c ON c.id=cr.id WHERE c.username=?",[&room],|r|r.get(0)).optional()?.flatten()
            } else {
                None
            };
            let body = json!({"id":id,"username":id,"name":name,"nickName":r.get::<_,Option<String>>(2)?.unwrap_or_default(),"remark":r.get::<_,Option<String>>(3)?.unwrap_or_default(),"owner":owner.as_ref().is_some_and(|v|v==&id),"coverage":"client_members"});
            conn.execute(
                "INSERT OR REPLACE INTO members VALUES(?,?,?,?,?)",
                params![p.source_id, room, id, name, body.to_string()],
            )?;
        }
    }
    // Contact names may change even when message shards do not.
    conn.execute("UPDATE conversations SET name=COALESCE((SELECT name FROM contacts c WHERE c.source_id=conversations.source_id AND c.id=conversations.peer),name) WHERE source_id=?",[&p.source_id])?;
    Ok(n)
}
fn columns(db: &Connection, table: &str) -> Result<Vec<String>> {
    let mut st = db.prepare(&format!(
        "PRAGMA table_info(\"{}\")",
        table.replace('"', "\"\"")
    ))?;
    Ok(st
        .query_map([], |r| r.get(1))?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn import_messages(
    conn: &Connection,
    p: &Profile,
    path: &Path,
    shard: &str,
    force: bool,
) -> Result<usize> {
    let mut sync = store::SyncSet::new(conn, p, shard, force)?;
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let names = names(conn, p)?;
    let mut id_map = BTreeMap::new();
    if table_exists(&db, "Name2Id") {
        let mut st = db.prepare("SELECT rowid,user_name FROM Name2Id")?;
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            id_map.insert(id, name);
        }
    }
    let mut table_map = BTreeMap::new();
    for name in id_map.values().chain(names.keys()) {
        table_map.insert(
            format!("Msg_{}", hex::encode(Md5::digest(name.as_bytes()))),
            name.clone(),
        );
    }
    let mut st = db.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
    let tables = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut n = 0;
    for table in tables {
        if table.len() != 36
            || !table.starts_with("Msg_")
            || !table[4..].bytes().all(|c| c.is_ascii_hexdigit())
        {
            continue;
        }
        let chat = table_map
            .get(&table)
            .cloned()
            .unwrap_or_else(|| table.clone());
        let name = names.get(&chat).cloned().unwrap_or(chat.clone());
        let kind = if chat.contains("@chatroom") {
            "group"
        } else {
            "buddy"
        };
        store::conversation(conn, p, &chat, &chat, kind, &name)?;
        let mut st=db.prepare(&format!("SELECT local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content FROM \"{table}\""))?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let local: i64 = row.get(0)?;
            let raw_type: i64 = row.get(1)?;
            let time: i64 = row.get(2)?;
            let sender = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
            let raw = bytes(row, 4);
            let compressed = row.get::<_, Option<i64>>(5)?.unwrap_or(0) == 4;
            let original = format!("{shard}:{table}:{local}");
            let id = store::message_id(p, &original);
            let signature = store::row_signature(&[
                b"wechat-parser-v2",
                &raw_type.to_le_bytes(),
                &time.to_le_bytes(),
                &sender.to_le_bytes(),
                &[compressed as u8],
                &raw,
            ]);
            let (parse, legacy) = sync.check(&id, &signature);
            if !parse {
                continue;
            }
            let (mut warnings, text) = (vec![], String::new());
            let text = match content(&raw, compressed) {
                Ok(s) => s,
                Err(_) => {
                    warnings.push("消息内容解压失败".into());
                    text
                }
            };
            let (sender_from_content, text, kind_msg, elements) =
                parse_message(&text, raw_type, kind == "group");
            let sender = id_map
                .get(&sender)
                .cloned()
                .filter(|s| s != &chat || kind != "group")
                .unwrap_or(sender_from_content);
            let sender_name = names.get(&sender).cloned().unwrap_or(sender.clone());
            let message = Message {
                id,
                original_id: original,
                source_id: p.source_id.clone(),
                platform: p.platform.clone(),
                account: p.account.clone(),
                conversation_id: chat.clone(),
                conversation_name: name.clone(),
                conversation_kind: kind.into(),
                time,
                sent_at: stamp(time),
                sender_id: sender,
                sender_name,
                kind: kind_msg,
                text,
                elements,
                decode_warnings: warnings,
                metadata: json!({"localId":local,"rawType":raw_type,"sourceDatabase":shard,"sourceTable":table}),
            };
            store::insert_message(conn, p, shard, &message, local, &signature, legacy)?;
            n += 1;
            if n % 50000 == 0 {
                eprintln!("微信已索引 {n} 条消息");
            }
        }
    }
    sync.finish(conn)?;
    Ok(n)
}
pub fn import_sessions(conn: &Connection, p: &Profile, path: &Path) -> Result<usize> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let names = names(conn, p)?;
    conn.execute("UPDATE conversations SET unread=0,last_time=0,last_text='',body='{}' WHERE source_id=? AND json_extract(body,'$.unreadSource')='client_snapshot'",[&p.source_id])?;
    let mut st=db.prepare("SELECT username,unread_count,summary,last_timestamp,last_msg_type,last_msg_sender,last_sender_display_name FROM SessionTable")?;
    let mut rows = st.query([])?;
    let mut n = 0;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let name = names.get(&id).cloned().unwrap_or(id.clone());
        let kind = if id.contains("@chatroom") {
            "group"
        } else {
            "buddy"
        };
        let data = bytes(row, 2);
        let text = content(&data, data.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]))
            .unwrap_or_else(|_| "(无法解压)".into());
        let unread = row.get::<_, Option<i64>>(1)?.unwrap_or(0);
        let time = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
        store::conversation(conn, p, &id, &id, kind, &name)?;
        conn.execute("UPDATE conversations SET unread=?,last_time=?,last_text=?,body=? WHERE source_id=? AND id=?",params![unread,time,text,json!({"unreadSource":"client_snapshot","rawType":row.get::<_,Option<i64>>(4)?,"lastSender":row.get::<_,Option<String>>(5)?,"lastSenderName":row.get::<_,Option<String>>(6)?}).to_string(),p.source_id,id])?;
        n += 1;
    }
    Ok(n)
}
pub fn import_favorites(conn: &Connection, p: &Profile, path: &Path) -> Result<usize> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let names = names(conn, p)?;
    conn.execute("DELETE FROM favorites WHERE source_id=?", [&p.source_id])?;
    let mut st = db.prepare(
        "SELECT local_id,type,update_time,content,fromusr,realchatname FROM fav_db_item",
    )?;
    let mut rows = st.query([])?;
    let mut n = 0;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let raw: i64 = row.get(1)?;
        let time: i64 = row.get(2)?;
        let text = String::from_utf8_lossy(&bytes(row, 3)).into_owned();
        let fields = xml_fields(&text);
        let get = |k: &str| fields.get(k).cloned().unwrap_or_default();
        let kind = match raw {
            1 => "text",
            2 => "image",
            5 => "article",
            19 => "card",
            20 => "video",
            _ => "unknown",
        };
        let summary = match raw {
            2 => "[图片收藏]".into(),
            5 => format!("{} {}", get("pagetitle"), get("pagedesc")),
            20 => format!("{} {}", get("nickname"), get("desc")),
            _ => get("desc"),
        };
        let from = row.get::<_, Option<String>>(4)?.unwrap_or_default();
        let chat = row.get::<_, Option<String>>(5)?.unwrap_or_default();
        let body = json!({"id":id.to_string(),"type":kind,"rawType":raw,"time":time,"summary":summary.trim(),"from":names.get(&from).unwrap_or(&from),"sourceChat":names.get(&chat).unwrap_or(&chat),"xmlFields":fields});
        conn.execute(
            "INSERT INTO favorites VALUES(?,?,?,?,?,?)",
            params![
                p.source_id,
                id.to_string(),
                kind,
                time,
                format!("{summary}\n{text}"),
                body.to_string()
            ],
        )?;
        n += 1;
    }
    Ok(n)
}
pub fn import(
    conn: &Connection,
    p: &Profile,
    path: &Path,
    shard: &str,
    force: bool,
) -> Result<usize> {
    let name = shard.replace('\\', "/");
    if name.ends_with("contact/contact.db") {
        import_contacts(conn, p, path)
    } else if name.ends_with("session/session.db") {
        import_sessions(conn, p, path)
    } else if name.ends_with("favorite/favorite.db") {
        import_favorites(conn, p, path)
    } else if name.contains("message_") && name.ends_with(".db") {
        import_messages(conn, p, path, shard, force)
    } else {
        Ok(0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_and_quote() {
        let (sender, text, kind, _) = parse_message("wxid_a:\n你好", 1, true);
        assert_eq!(sender, "wxid_a");
        assert_eq!(text, "你好");
        assert_eq!(kind, "text");
        let (_, text, kind, _) = parse_message(
            "<msg><appmsg><type>57</type><title>答复</title><refermsg><displayname>A</displayname><content>原文</content></refermsg></appmsg></msg>",
            49,
            false,
        );
        assert!(text.contains("原文"));
        assert_eq!(kind, "link");
    }
    #[test]
    fn zstd() {
        let raw = zstd::stream::encode_all("测试消息".as_bytes(), 1).unwrap();
        assert_eq!(content(&raw, true).unwrap(), "测试消息");
        assert!(content(b"bad", true).is_err());
    }
    #[test]
    fn legacy_system_labels() {
        assert_eq!(parse_message("提示", 10000, false).1, "[系统] 提示");
        assert_eq!(parse_message("已撤回", 10002, false).1, "[撤回] 已撤回");
    }
}
