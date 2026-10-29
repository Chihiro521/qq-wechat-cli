use crate::{
    model::{Message, Profile, stamp},
    store,
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::path::Path;

fn utf16(bytes: &[u8]) -> String {
    String::from_utf16_lossy(
        &bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<_>>(),
    )
    .trim_start_matches('\u{feff}')
    .into()
}
fn tlv<'a>(data: &'a [u8], warnings: &mut Vec<String>) -> Vec<(u8, &'a [u8])> {
    let mut out = vec![];
    let mut pos = 0;
    while pos < data.len() {
        if pos + 3 > data.len() {
            warnings.push(format!("TLV 头被截断，offset={pos}"));
            break;
        }
        let tag = data[pos];
        let len = u16::from_le_bytes([data[pos + 1], data[pos + 2]]) as usize;
        pos += 3;
        if pos + len > data.len() {
            warnings.push(format!("TLV 长度超出剩余字节，tag={tag}"));
            break;
        }
        out.push((tag, &data[pos..pos + len]));
        pos += len;
    }
    out
}
pub fn decode(blob: &[u8]) -> (String, String, String, Vec<Value>, Vec<String>, Vec<u8>) {
    let mut warnings = vec![];
    let mut elements = vec![];
    let mut texts = vec![];
    let mut names = vec![];
    let mut unknown = vec![];
    if blob.len() < 26 {
        return (
            "".into(),
            "unknown".into(),
            "".into(),
            elements,
            vec!["消息头被截断".into()],
            unknown,
        );
    }
    let font = u16::from_le_bytes([blob[24], blob[25]]) as usize;
    let offset = 28 + font;
    if offset > blob.len() {
        return (
            "".into(),
            "unknown".into(),
            "".into(),
            elements,
            vec!["字体字段被截断".into()],
            unknown,
        );
    }
    for (tag, bytes) in tlv(&blob[offset..], &mut warnings) {
        if ![1, 2, 3, 6, 7, 18, 26].contains(&tag) {
            if !unknown.contains(&tag) {
                unknown.push(tag);
            }
            continue;
        }
        let parts = tlv(bytes, &mut warnings);
        let first = |tag| parts.iter().find(|x| x.0 == tag).map(|x| x.1);
        match tag {
            1 => {
                for (_, text) in parts.iter().filter(|p| p.0 == 1) {
                    let text = utf16(text);
                    elements.push(json!({"type":"text","text":text}));
                    texts.push(text);
                }
            }
            2 => {
                if let Some(data) = first(1) {
                    let id = data
                        .iter()
                        .fold(0u128, |v, b| v.wrapping_shl(8) | *b as u128)
                        .to_string();
                    elements.push(json!({"type":"face","id":id}));
                    texts.push(format!("[表情:{id}]"));
                }
            }
            3 | 6 => {
                elements.push(json!({"type":"image","scope":if tag==3{"group"}else{"private"},"hash":first(1).map(hex::encode),"path":first(2).map(utf16)}));
                texts.push("[图片]".into());
            }
            7 => {
                elements.push(json!({"type":"voice","hash":first(1).map(hex::encode)}));
                texts.push("[语音]".into());
            }
            18 => {
                for (kind, data) in &parts {
                    if *kind == 1 || *kind == 2 {
                        let name = utf16(data).trim().to_string();
                        if !name.is_empty() {
                            names.push(name);
                        }
                    }
                }
            }
            26 => {
                let hash = first(1)
                    .filter(|b| b.len() >= 260)
                    .map(|b| hex::encode(b[244..260].iter().map(|x| x ^ 0xef).collect::<Vec<_>>()));
                elements.push(json!({"type":"video","hash":hash}));
                texts.push("[视频]".into());
            }
            _ => {}
        }
    }
    if texts.is_empty() && !unknown.is_empty() {
        texts.push(format!(
            "[QQ消息:{}]",
            unknown
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    let mut kinds = elements
        .iter()
        .map(|e| e["type"].as_str().unwrap_or("unknown"))
        .collect::<Vec<_>>();
    kinds.sort();
    kinds.dedup();
    let kind = match kinds.len() {
        0 => "unknown",
        1 => kinds[0],
        _ => "mixed",
    }
    .to_string();
    (
        texts.concat(),
        kind,
        names.pop().unwrap_or_default(),
        elements,
        warnings,
        unknown,
    )
}
pub fn receiver_names(info: &[u8]) -> Vec<String> {
    static MARKER: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let marker = MARKER.get_or_init(|| {
        let name: Vec<u8> = "strRecieverShowName"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut marker = vec![8, name.len() as u8, 0];
        marker.extend(name.iter().map(|v| v ^ (255 - name.len() as u8)));
        marker
    });
    let mut names = vec![];
    let mut pos = 0;
    while pos + marker.len() + 4 <= info.len() {
        let Some(i) = info[pos..].windows(marker.len()).position(|w| w == marker) else {
            break;
        };
        let start = pos + i + marker.len();
        let len = u32::from_le_bytes(info[start..start + 4].try_into().unwrap()) as usize;
        let value = start + 4;
        if len > 0 && len <= 255 && len % 2 == 0 && value + len <= info.len() {
            let bytes: Vec<_> = info[value..value + len]
                .iter()
                .map(|v| v ^ (255 - len as u8))
                .collect();
            let text = utf16(&bytes);
            if !text.is_empty() && !text.chars().any(char::is_control) {
                names.push(text);
            }
        }
        pos = value.saturating_add(len).max(pos + 1);
    }
    names
}
fn table_id(table: &str) -> Option<(String, String, String)> {
    let (kind, peer) = table.split_once('_')?;
    if !["group", "buddy", "system"].contains(&kind)
        || peer.is_empty()
        || !peer.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some((format!("{kind}:{peer}"), kind.into(), peer.into()))
}
pub fn import(
    conn: &Connection,
    p: &Profile,
    path: &Path,
    shard: &str,
    pending_byte: u32,
    force: bool,
) -> Result<usize> {
    use crate::sqlite_compat::{PendingByte, standard};
    let mut sync = store::SyncSet::new(conn, p, shard, force)?;
    let _source_guard = PendingByte::new(pending_byte);
    let source = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.execute_batch("PRAGMA cache_size=-16384; PRAGMA mmap_size=134217728;")?;
    let mut list = source.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
    let tables = list
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut count = 0;
    let mut supported = 0;
    for table in tables {
        let Some((chat, kind, peer)) = table_id(&table) else {
            continue;
        };
        supported += 1;
        let mut name = format!(
            "{} {peer}",
            if kind == "group" {
                "群聊"
            } else if kind == "buddy" {
                "私聊"
            } else {
                "系统"
            }
        );
        let mut latest_name: Option<(i64, i64)> = None;
        standard(|| store::conversation(conn, p, &chat, &peer, &kind, &name))?;
        // Find the latest usable display name during the message scan. Sorting every
        // Info blob by an unsigned expression spills gigabytes to a temporary file.
        let mut st=source.prepare(&format!("SELECT (Time & 4294967295),(Rand & 4294967295),(SenderUin & 4294967295),MsgContent,Info FROM \"{table}\" ORDER BY rowid DESC"))?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let time: i64 = row.get(0)?;
            let rand: i64 = row.get(1)?;
            if kind != "system" && latest_name.is_none_or(|latest| (time, rand) >= latest) {
                if let Ok(data) = row.get::<_, Vec<u8>>(4) {
                    if let Some(value) = receiver_names(&data).into_iter().rev().find(|n| {
                        !n.trim().is_empty() && n != &peer && !n.chars().all(|c| c.is_ascii_digit())
                    }) {
                        latest_name = Some((time, rand));
                        name = value;
                    }
                }
            }
            let sender = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let blob = row.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default();
            let original = if kind == "group" {
                format!("{peer}:{time}:{rand}")
            } else {
                format!("{kind}:{peer}:{time}:{rand}")
            };
            let id = store::message_id(p, &original);
            let signature = store::row_signature(&[
                &time.to_le_bytes(),
                &rand.to_le_bytes(),
                &sender.to_le_bytes(),
                &blob,
            ]);
            let (parse, legacy) = sync.check(&id, &signature);
            if !parse {
                continue;
            }
            let (text, mut msg_type, sender_name, elements, mut warnings, unknown) = decode(&blob);
            if blob.len() >= 16 {
                if u32::from_le_bytes(blob[8..12].try_into().unwrap()) as i64 != time {
                    warnings.push("消息头与数据库时间不一致".into());
                }
                if u32::from_le_bytes(blob[12..16].try_into().unwrap()) as i64 != rand {
                    warnings.push("消息头与数据库 Rand 不一致".into());
                }
            }
            if kind == "system" && msg_type == "unknown" {
                msg_type = "system".into();
            }
            let message = Message {
                id,
                original_id: original,
                source_id: p.source_id.clone(),
                platform: p.platform.clone(),
                account: p.account.clone(),
                conversation_id: chat.clone(),
                conversation_name: name.clone(),
                conversation_kind: kind.clone(),
                time,
                sent_at: stamp(time),
                sender_id: sender.to_string(),
                sender_name: if sender_name.is_empty() {
                    sender.to_string()
                } else {
                    sender_name
                },
                kind: msg_type,
                text,
                elements,
                decode_warnings: warnings,
                metadata: json!({"rand":rand,"unknownTags":unknown}),
            };
            standard(|| store::insert_message(conn, p, shard, &message, rand, &signature, legacy))?;
            count += 1;
            if count % 50000 == 0 {
                eprintln!("QQ 已索引 {count} 条消息");
            }
        }
        standard(|| store::conversation(conn, p, &chat, &peer, &kind, &name))?;
        if kind == "buddy" {
            standard(|| {
                conn.execute(
                    "INSERT OR REPLACE INTO contacts VALUES(?,?,?,?)",
                    params![
                        p.source_id,
                        peer,
                        name,
                        json!({"id":peer,"name":name,"coverage":"observed_correspondents"})
                            .to_string()
                    ],
                )
            })?;
        }
    }
    ensure!(
        supported > 0,
        "DATABASE_SCHEMA_UNSUPPORTED: 没有可识别的 Classic QQ 消息表"
    );
    standard(|| sync.finish(conn))?;
    Ok(count)
}
pub fn parse_favorite(data: &[u8]) -> Value {
    let mut texts = vec![];
    for bytes in data.split(|b| *b < 32) {
        if let Ok(s) = std::str::from_utf8(bytes) {
            let s = s.trim();
            if s.chars().count() >= 3 {
                texts.push(s.to_string());
            }
        }
    }
    for bytes in data
        .chunks_exact(2)
        .collect::<Vec<_>>()
        .split(|b| u16::from_le_bytes([b[0], b[1]]) < 32)
    {
        let b: Vec<u8> = bytes.iter().flat_map(|b| b.iter().copied()).collect();
        let s = utf16(&b).trim().to_string();
        if s.chars().count() >= 3 && !s.contains('\u{fffd}') {
            texts.push(s);
        }
    }
    texts.sort();
    texts.dedup();
    let mut fields = serde_json::Map::new();
    let allowed = [
        "title",
        "summary",
        "description",
        "desc",
        "content",
        "text",
        "name",
        "source",
        "author",
        "url",
    ];
    for text in &texts {
        if let Ok(xml) = roxmltree::Document::parse(text) {
            for node in xml.descendants().filter(|n| n.is_element()) {
                if allowed.contains(&node.tag_name().name()) {
                    if let Some(v) = node.text() {
                        fields
                            .entry(node.tag_name().name())
                            .or_insert(Value::String(v.trim().into()));
                    }
                }
            }
        }
    }
    let summary = allowed
        .iter()
        .filter_map(|k| fields.get(*k).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" | ");
    let summary = if summary.is_empty() {
        texts.join(" | ")
    } else {
        summary
    };
    json!({"summary":summary.chars().take(500).collect::<String>(),"sourceText":texts.join("\n"),"xmlFields":fields,"coverage":if fields.is_empty(){"text"}else{"structured"}})
}
pub fn favorites(conn: &Connection, p: &Profile, path: &Path) -> Result<usize> {
    fn scalar(row: &rusqlite::Row, index: usize) -> Result<Value> {
        use rusqlite::types::ValueRef;
        Ok(match row.get_ref(index)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(v) => json!(v),
            ValueRef::Real(v) => json!(v),
            ValueRef::Text(v) | ValueRef::Blob(v) => json!(String::from_utf8_lossy(v)),
        })
    }
    fn timestamp(raw: &Value) -> i64 {
        let numeric = raw
            .as_f64()
            .or_else(|| raw.as_str().and_then(|v| v.parse::<f64>().ok()));
        if let Some(n) = numeric {
            if n.is_finite() && n > 0.0 {
                return if n >= 100_000_000_000.0 {
                    (n / 1000.0) as i64
                } else {
                    n as i64
                };
            }
        }
        if let Some(s) = raw.as_str() {
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
                return t.timestamp();
            }
            if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
                use chrono::TimeZone;
                return chrono::Local
                    .from_local_datetime(&t)
                    .single()
                    .map(|t| t.timestamp())
                    .unwrap_or(0);
            }
        }
        0
    }
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut st=db.prepare("SELECT lid,cid,type,localTime,serverTime,createTime,collectTime,data FROM Cid2DataTable ORDER BY collectTime DESC,lid DESC")?;
    let mut rows = st.query([])?;
    let mut n = 0;
    conn.execute("DELETE FROM favorites WHERE source_id=?", [&p.source_id])?;
    while let Some(row) = rows.next()? {
        let raw_id = scalar(row, 0)?;
        let id = raw_id
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| raw_id.to_string());
        let raw = scalar(row, 2)?;
        let numeric = raw
            .as_i64()
            .or_else(|| raw.as_str().and_then(|s| s.parse::<i64>().ok()));
        let collect = scalar(row, 6)?;
        let time = timestamp(&collect);
        let data = match row.get_ref(7)? {
            rusqlite::types::ValueRef::Blob(v) | rusqlite::types::ValueRef::Text(v) => v.to_vec(),
            _ => vec![],
        };
        let mut parsed = parse_favorite(&data);
        let kind = match numeric {
            Some(1) => "text",
            Some(2) => "image",
            Some(3) => "article",
            Some(4) => "card",
            Some(5) => "video",
            _ => raw
                .as_str()
                .filter(|s| ["text", "image", "article", "card", "video"].contains(s))
                .unwrap_or("unknown"),
        };
        let mut timestamps = serde_json::Map::new();
        for (index, name) in [
            (3, "localTime"),
            (4, "serverTime"),
            (5, "createTime"),
            (6, "collectTime"),
        ] {
            let value = scalar(row, index)?;
            let at = timestamp(&value);
            timestamps.insert(name.into(), value);
            timestamps.insert(
                format!("{name}At"),
                if at > 0 {
                    json!(stamp(at))
                } else {
                    Value::Null
                },
            );
        }
        parsed["id"] = json!(id);
        parsed["lid"] = raw_id;
        parsed["cid"] = scalar(row, 1)?;
        parsed["type"] = json!(kind);
        parsed["rawType"] = raw.clone();
        parsed["time"] = json!(time);
        parsed["timestamps"] = json!(timestamps);
        conn.execute(
            "INSERT INTO favorites VALUES(?,?,?,?,?,?)",
            params![
                p.source_id,
                id,
                kind,
                time,
                parsed["sourceText"].as_str().unwrap_or(""),
                parsed.to_string()
            ],
        )?;
        n += 1;
    }
    Ok(n)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receiver_marker_roundtrip() {
        let name = "测试群";
        let field: Vec<u8> = "strRecieverShowName"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let value: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut data = vec![8, field.len() as u8, 0];
        data.extend(field.iter().map(|v| v ^ (255 - field.len() as u8)));
        data.extend((value.len() as u32).to_le_bytes());
        data.extend(value.iter().map(|v| v ^ (255 - value.len() as u8)));
        assert_eq!(receiver_names(&data), vec![name]);
    }
    #[test]
    fn truncation_is_reported() {
        assert!(!decode(&[0; 3]).4.is_empty());
    }
}
