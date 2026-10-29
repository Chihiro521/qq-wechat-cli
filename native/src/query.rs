use crate::{
    model::{Profile, flag, number, positional, string},
    store,
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::io::{BufWriter, Write};

fn value_rows(
    conn: &Connection,
    sql: &str,
    values: &[rusqlite::types::Value],
) -> Result<Vec<Value>> {
    let mut st = conn.prepare(sql)?;
    let rows = st.query_map(rusqlite::params_from_iter(values), |r| {
        r.get::<_, String>(0)
    })?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}
fn selector<'a>(args: &'a Value) -> Result<&'a str> {
    positional(args, 0).ok_or_else(|| anyhow::anyhow!("INVALID_ARGUMENT: 请指定会话"))
}
pub fn sessions(conn: &Connection, p: &Profile, args: &Value, unread_only: bool) -> Result<Value> {
    let limit = number(args, "limit", 20);
    ensure!(limit <= 10000, "INVALID_ARGUMENT: limit 过大");
    let query = string(args, "query").unwrap_or("").to_lowercase();
    let kind = string(args, "kind").unwrap_or("all");
    ensure!(
        ["all", "group", "buddy", "system"].contains(&kind),
        "INVALID_ARGUMENT: kind 无效"
    );
    let mut st=conn.prepare("SELECT c.id,c.peer,c.kind,c.name,c.unread,c.last_time,c.last_text,c.body,c.message_count FROM conversations c WHERE c.source_id=? ORDER BY c.last_time DESC,c.id")?;
    let mut rows = st.query([&p.source_id])?;
    let mut result = vec![];
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let name: String = row.get(3)?;
        let conversation_kind: String = row.get(2)?;
        if kind != "all" && kind != conversation_kind {
            continue;
        }
        if !query.is_empty()
            && !name.to_lowercase().contains(&query)
            && !id.to_lowercase().contains(&query)
        {
            continue;
        }
        let details: Value = serde_json::from_str(&row.get::<_, String>(7)?)?;
        if p.platform == "wechat"
            && p.files.contains_key("session/session.db")
            && details["unreadSource"] != "client_snapshot"
        {
            continue;
        }
        let unread: i64 = if p.platform == "wechat" {
            row.get(4)?
        } else {
            conn.query_row("SELECT count(*) FROM messages m JOIN checkpoints cp ON cp.source_id=m.source_id AND cp.conversation_id=m.conversation_id WHERE m.source_id=? AND m.conversation_id=? AND (m.time,m.sequence,m.id)>(cp.time,cp.sequence,cp.id)",params![p.source_id,id],|r|r.get(0))?
        };
        if unread_only && unread <= 0 {
            continue;
        }
        result.push(json!({"id":id,"peerId":row.get::<_,String>(1)?,"kind":conversation_kind,"name":name,"unread":unread,"unreadSource":if p.platform=="qq"{"cli_checkpoint"}else{"client_snapshot"},"lastMessageTime":row.get::<_,i64>(5)?,"lastMessage":row.get::<_,String>(6)?,"messageCount":row.get::<_,i64>(8)?,"metadata":serde_json::from_str::<Value>(&row.get::<_,String>(7)?)?}));
        if result.len() >= limit {
            break;
        }
    }
    if limit == 0 {
        result.clear();
    }
    Ok(json!({"count":result.len(),"sessions":result}))
}
pub fn contacts(conn: &Connection, p: &Profile, args: &Value) -> Result<Value> {
    if let Some(detail) = string(args, "detail") {
        let rows = value_rows(
            conn,
            "SELECT body FROM contacts WHERE source_id=? AND (id=? OR name=?)",
            &[
                p.source_id.clone().into(),
                detail.to_string().into(),
                detail.to_string().into(),
            ],
        )?;
        ensure!(!rows.is_empty(), "CONTACT_NOT_FOUND");
        ensure!(rows.len() == 1, "CONTACT_AMBIGUOUS: 请使用联系人 ID");
        return Ok(rows.into_iter().next().unwrap());
    }
    let query = string(args, "query").unwrap_or("");
    let limit = number(args, "limit", 50).min(10000);
    let rows = value_rows(
        conn,
        "SELECT body FROM contacts WHERE source_id=? AND (instr(lower(name),lower(?))>0 OR instr(lower(id),lower(?))>0) ORDER BY name,id LIMIT ?",
        &[
            p.source_id.clone().into(),
            query.to_string().into(),
            query.to_string().into(),
            (limit as i64).into(),
        ],
    )?;
    Ok(
        json!({"count":rows.len(),"contacts":rows,"coverage":if p.platform=="qq"{"observed_correspondents"}else{"client_contacts"}}),
    )
}
pub fn members(conn: &Connection, p: &Profile, args: &Value) -> Result<Value> {
    let conversation = store::resolve(conn, p, selector(args)?)?;
    ensure!(
        conversation["kind"] == "group",
        "INVALID_ARGUMENT: 请选择群聊"
    );
    let id = conversation["id"].as_str().unwrap();
    let limit = number(args, "limit", 50).min(100000);
    let rows = if p.platform == "wechat" {
        value_rows(
            conn,
            "SELECT body FROM members WHERE source_id=? AND conversation_id=? ORDER BY json_extract(body,'$.owner') DESC,name,id LIMIT ?",
            &[
                p.source_id.clone().into(),
                id.to_string().into(),
                (limit as i64).into(),
            ],
        )?
    } else {
        let mut st=conn.prepare("SELECT sender_id,json_extract(body,'$.senderName'),count(*) FROM messages WHERE source_id=? AND conversation_id=? GROUP BY sender_id ORDER BY count(*) DESC,sender_id LIMIT ?")?;
        st.query_map(params![p.source_id,id,limit as i64],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"messageCount":r.get::<_,i64>(2)?,"coverage":"observed_senders"})))?.collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(
        json!({"conversation":conversation,"count":rows.len(),"members":rows,"coverage":if p.platform=="qq"{"observed_senders"}else{"client_members"}}),
    )
}
pub fn favorites(conn: &Connection, p: &Profile, args: &Value) -> Result<Value> {
    let query = string(args, "query").unwrap_or("");
    let mut values = vec![p.source_id.clone().into(), query.to_string().into()];
    let mut sql = "SELECT body FROM favorites WHERE source_id=? AND instr(lower(text),lower(?))>0"
        .to_string();
    if let Some(kind) = args
        .get("type")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(Value::as_str)
    {
        sql.push_str(" AND kind=?");
        values.push(kind.to_string().into());
    }
    sql.push_str(" ORDER BY time DESC,id LIMIT ?");
    values.push((number(args, "limit", 20).min(10000) as i64).into());
    let rows = value_rows(conn, &sql, &values)?;
    Ok(
        json!({"count":rows.len(),"favorites":rows,"coverage":if p.platform=="qq"{"partial_text_xml"}else{"client_favorites"}}),
    )
}
fn decorate(conn: &Connection, p: &Profile, message: &mut Value, media: bool) {
    if let Some(id) = message["conversationId"].as_str() {
        if let Ok(name) = conn
            .prepare_cached("SELECT name FROM conversations WHERE source_id=? AND id=?")
            .and_then(|mut st| st.query_row(params![p.source_id, id], |r| r.get::<_, String>(0)))
        {
            message["conversationName"] = json!(name);
        }
    }
    if p.platform == "wechat" {
        if let Some(id) = message["senderId"].as_str() {
            if let Ok(name) = conn
                .prepare_cached("SELECT name FROM contacts WHERE source_id=? AND id=?")
                .and_then(|mut st| {
                    st.query_row(params![p.source_id, id], |r| r.get::<_, String>(0))
                })
            {
                message["senderName"] = json!(name);
            }
        }
    }
    if !media {
        return;
    }
    let time = message["time"].as_i64().unwrap_or(0);
    let month = chrono::DateTime::from_timestamp(time, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m").to_string())
        .unwrap_or_default();
    let chat = message["conversationId"].as_str().unwrap_or("").to_owned();
    if let Some(elements) = message["elements"].as_array_mut() {
        for element in elements {
            if let Some(path) = element["path"].as_str() {
                let path = std::path::Path::new(path);
                let full = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    p.source.parent().unwrap_or(&p.source).join(path)
                };
                element["resolvedPath"] = json!(full);
                element["exists"] = json!(full.exists());
                continue;
            }
            if p.platform == "wechat" && element["type"] == "file" {
                if let Some(name) = element["filename"].as_str() {
                    let name_path = std::path::Path::new(name);
                    if !name.is_empty()
                        && name_path.file_name().is_some_and(|n| n == name)
                        && !name.contains(['\\', '/'])
                    {
                        let path = p
                            .source
                            .parent()
                            .unwrap_or(&p.source)
                            .join("msg/file")
                            .join(&month)
                            .join(name);
                        element["path"] = json!(path);
                        element["exists"] = json!(path.is_file());
                    }
                }
            }
            if p.platform == "wechat"
                && ["image", "voice", "video"]
                    .iter()
                    .any(|k| element["type"] == *k)
            {
                use md5::{Digest, Md5};
                let sub = match element["type"].as_str().unwrap_or("") {
                    "image" => "Img",
                    "voice" => "Voice",
                    _ => "Video",
                };
                let dir = p
                    .source
                    .parent()
                    .unwrap_or(&p.source)
                    .join("msg/attach")
                    .join(hex::encode(Md5::digest(chat.as_bytes())))
                    .join(&month)
                    .join(sub);
                let hash = element["fields"]["img@md5"]
                    .as_str()
                    .or_else(|| element["fields"]["videomsg@md5"].as_str());
                let file = hash
                    .filter(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                    .and_then(|hash| {
                        std::fs::read_dir(&dir)
                            .ok()?
                            .filter_map(Result::ok)
                            .find(|e| {
                                e.file_name()
                                    .to_string_lossy()
                                    .to_lowercase()
                                    .contains(&hash.to_lowercase())
                            })
                            .map(|e| e.path())
                    });
                if let Some(file) = file {
                    element["path"] = json!(file);
                    element["exists"] = json!(true);
                } else {
                    element["searchDirectory"] = json!(dir);
                    element["exists"] = json!(false);
                }
            }
        }
    }
}
pub fn history(conn: &Connection, p: &Profile, args: &Value, search: bool) -> Result<Value> {
    let q = if search {
        store::message_query(conn, p, args, None, Some(selector(args)?))?
    } else {
        store::message_query(conn, p, args, Some(selector(args)?), None)?
    };
    let mut data = store::messages(conn, &q)?;
    if let Some(rows) = data["messages"].as_array_mut() {
        for row in rows {
            decorate(conn, p, row, flag(args, "media"));
        }
    }
    Ok(data)
}
pub fn statistics(conn: &Connection, p: &Profile, args: &Value) -> Result<Value> {
    let conversation = store::resolve(conn, p, selector(args)?)?;
    let q = store::message_query(conn, p, args, Some(selector(args)?), None)?;
    let base = q.sql.split(" ORDER BY ").next().unwrap();
    let (total, first, last): (i64, Option<i64>, Option<i64>) = conn.query_row(
        &format!("SELECT count(*),min(m.time),max(m.time) {base}"),
        rusqlite::params_from_iter(&q.values),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut st = conn.prepare(&format!(
        "SELECT m.kind,count(*) {base} GROUP BY m.kind ORDER BY count(*) DESC"
    ))?;
    let types = st
        .query_map(rusqlite::params_from_iter(&q.values), |r| {
            Ok(json!({"type":r.get::<_,String>(0)?,"count":r.get::<_,i64>(1)?}))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut values = q.values.clone();
    values.push((number(args, "top", 10).min(1000) as i64).into());
    let mut st=conn.prepare(&format!("SELECT m.sender_id,json_extract(m.body,'$.senderName'),count(*) {base} GROUP BY m.sender_id ORDER BY count(*) DESC,m.sender_id LIMIT ?"))?;
    let senders=st.query_map(rusqlite::params_from_iter(&values),|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"count":r.get::<_,i64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(
        json!({"conversation":conversation,"totalMessages":total,"firstMessageTime":first,"lastMessageTime":last,"types":types,"topSenders":senders}),
    )
}
pub fn summarize(conn: &Connection, p: &Profile, args: &Value) -> Result<Value> {
    let mut args = args.clone();
    if args.get("limit").is_none() {
        args["limit"] = json!(500);
    }
    let data = history(conn, p, &args, false)?;
    let messages = data["messages"].as_array().unwrap();
    let size = number(&args, "chunk-size", 80);
    ensure!(
        size > 0 && size <= 1000,
        "INVALID_ARGUMENT: chunk-size 范围为 1..1000"
    );
    let chunks=messages.chunks(size).enumerate().map(|(n,rows)|json!({"index":n,"messageIds":rows.iter().map(|m|m["id"].clone()).collect::<Vec<_>>(),"messages":rows,"text":rows.iter().map(|m|format!("[{}] {}: {} [ref:{}]",m["sentAt"].as_str().unwrap_or(""),m["senderName"].as_str().unwrap_or(""),m["text"].as_str().unwrap_or(""),m["id"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")})).collect::<Vec<_>>();
    Ok(
        json!({"mode":"local_evidence","prompt":string(&args,"prompt").unwrap_or("总结主要话题，并引用消息标识"),"count":messages.len(),"chunks":chunks}),
    )
}
pub fn new_messages(conn: &mut Connection, p: &Profile, args: &Value) -> Result<Value> {
    let tx = conn.transaction()?;
    let baseline: i64 = tx.query_row(
        "SELECT count(*) FROM checkpoints WHERE source_id=?",
        [&p.source_id],
        |r| r.get(0),
    )?;
    let mut messages = vec![];
    if baseline > 0 {
        let limit = number(args, "limit", 500).min(100000);
        let sql = "SELECT m.body FROM messages m LEFT JOIN checkpoints c ON c.source_id=m.source_id AND c.conversation_id=m.conversation_id WHERE m.source_id=? AND (c.id IS NULL OR (m.time,m.sequence,m.id)>(c.time,c.sequence,c.id)) ORDER BY m.time ASC,m.sequence ASC,m.id ASC LIMIT ?";
        messages = value_rows(
            &tx,
            sql,
            &[p.source_id.clone().into(), (limit as i64).into()],
        )?;
    }
    if baseline == 0 {
        let mut st = tx.prepare("SELECT id FROM conversations WHERE source_id=?")?;
        let chats = st
            .query_map([&p.source_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(st);
        for chat in chats {
            let latest:Option<(i64,i64,String)>=tx.query_row("SELECT time,sequence,id FROM messages WHERE source_id=? AND conversation_id=? ORDER BY time DESC,sequence DESC,id DESC LIMIT 1",params![p.source_id,chat],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((time, sequence, id)) = latest {
                tx.execute(
                    "INSERT OR REPLACE INTO checkpoints VALUES(?,?,?,?,?)",
                    params![p.source_id, chat, time, sequence, id],
                )?;
            }
        }
    } else {
        for m in &messages {
            let id = m["id"].as_str().unwrap();
            tx.execute("INSERT INTO checkpoints SELECT source_id,conversation_id,time,sequence,id FROM messages WHERE id=? ON CONFLICT(source_id,conversation_id) DO UPDATE SET time=excluded.time,sequence=excluded.sequence,id=excluded.id WHERE (excluded.time,excluded.sequence,excluded.id)>(checkpoints.time,checkpoints.sequence,checkpoints.id)",[id])?;
        }
    }
    tx.commit()?;
    for m in &mut messages {
        decorate(conn, p, m, flag(args, "media"));
    }
    Ok(json!({"baselineEstablished":baseline==0,"count":messages.len(),"messages":messages}))
}
pub fn export(conn: &Connection, p: &Profile, args: &Value) -> Result<()> {
    let conversation = store::resolve(conn, p, selector(args)?)?;
    let all = args.get("limit").is_none();
    let mut args = args.clone();
    args["order"] = json!("asc");
    let q = store::message_query(conn, p, &args, Some(selector(&args)?), None)?;
    let mut output = BufWriter::new(std::io::stdout().lock());
    let mut meta = store::metadata(p);
    meta["conversationName"] = conversation["name"].clone();
    meta["conversationKind"] = conversation["kind"].clone();
    meta["peerId"] = conversation["peerId"].clone();
    meta["sourceSessionId"] = json!(format!(
        "{}-{}-{}-{}",
        p.platform,
        p.account,
        conversation["kind"].as_str().unwrap(),
        conversation["peerId"].as_str().unwrap()
    ));
    let emit = |out: &mut BufWriter<std::io::StdoutLock<'_>>, frame: &Value| -> Result<()> {
        serde_json::to_writer(&mut *out, frame)?;
        out.write_all(b"\n")?;
        Ok(())
    };
    emit(
        &mut output,
        &json!({"protocol":1,"kind":"start","meta":meta}),
    )?;
    output.flush()?;
    let mut values = q.values.clone();
    values.extend([
        (if all { -1 } else { q.limit as i64 }).into(),
        (q.offset as i64).into(),
    ]);
    if string(&args, "format") == Some("chatlab") {
        let mut members = conn.prepare(&format!("SELECT json_extract(body,'$.senderId'),json_extract(body,'$.senderName') FROM (SELECT m.body {} LIMIT ? OFFSET ?) GROUP BY json_extract(body,'$.senderId') ORDER BY json_extract(body,'$.senderId')",q.sql))?;
        let mut rows = members.query(rusqlite::params_from_iter(&values))?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let mut name: String = row.get(1)?;
            if p.platform == "wechat" {
                name = conn
                    .query_row(
                        "SELECT name FROM contacts WHERE source_id=? AND id=?",
                        params![p.source_id, id],
                        |r| r.get(0),
                    )
                    .unwrap_or(name);
            }
            emit(
                &mut output,
                &json!({"protocol":1,"kind":"member","data":{"platformId":id,"accountName":name,"groupNickname":name}}),
            )?;
        }
    }
    let mut st = conn.prepare(&format!("SELECT m.body {} LIMIT ? OFFSET ?", q.sql))?;
    let mut rows = st.query(rusqlite::params_from_iter(values))?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let mut m: Value = serde_json::from_str(&row.get::<_, String>(0)?)?;
        decorate(conn, p, &mut m, flag(&args, "media"));
        emit(&mut output, &json!({"protocol":1,"kind":"row","data":m}))?;
        count += 1;
    }
    meta["peakMemoryBytes"] = json!(crate::windows::peak_memory_bytes());
    emit(
        &mut output,
        &json!({"protocol":1,"kind":"end","ok":true,"data":{"count":count},"meta":meta}),
    )?;
    output.flush()?;
    Ok(())
}
