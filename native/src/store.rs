use crate::{
    crypto,
    model::{Message, Profile, Request, number, string},
};
use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn home(request: &Request) -> Result<PathBuf> {
    let path = request
        .data_dir
        .clone()
        .or_else(|| std::env::var_os("QQ_WECHAT_HOME").map(PathBuf::from))
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("LOCALAPPDATA")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .unwrap_or_else(|| ".".into()),
            )
            .join("qq-wechat-cli")
        });
    fs::create_dir_all(&path)?;
    canonical(&path)
}
pub fn canonical(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    let text = path.to_string_lossy();
    Ok(if let Some(p) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{p}"))
    } else {
        PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
    })
}
pub fn open(home: &Path) -> Result<Connection> {
    let conn = Connection::open(home.join("index.sqlite"))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    // Cached reads must not try to write the schema while a refresh transaction runs.
    // In particular INSERT OR IGNORE still obtains SQLite's writer lock.
    let existing = conn
        .query_row("SELECT value FROM settings WHERE key='schema'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok();
    if existing.as_deref() == Some("3") {
        conn.set_prepared_statement_cache_capacity(64);
        return Ok(conn);
    }
    ensure!(
        existing.as_deref().is_none_or(|v| v == "1" || v == "2"),
        "SCHEMA_UNSUPPORTED: 索引版本需要迁移"
    );
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
INSERT OR IGNORE INTO settings VALUES('schema','3');
CREATE TABLE IF NOT EXISTS profiles(source_id TEXT PRIMARY KEY,platform TEXT NOT NULL,account TEXT NOT NULL,body TEXT NOT NULL,UNIQUE(platform,account));
CREATE TABLE IF NOT EXISTS messages(rid INTEGER PRIMARY KEY,id TEXT NOT NULL UNIQUE,source_id TEXT NOT NULL,shard TEXT NOT NULL,conversation_id TEXT NOT NULL,time INTEGER NOT NULL,sequence INTEGER NOT NULL,sender_id TEXT NOT NULL,kind TEXT NOT NULL,text TEXT NOT NULL,body TEXT NOT NULL,signature TEXT NOT NULL DEFAULT '');
CREATE INDEX IF NOT EXISTS message_chat_time ON messages(source_id,conversation_id,time DESC,sequence DESC,id DESC);
CREATE INDEX IF NOT EXISTS message_time ON messages(source_id,time DESC,sequence DESC,id DESC);
CREATE INDEX IF NOT EXISTS message_shard ON messages(source_id,shard);
CREATE INDEX IF NOT EXISTS message_sender ON messages(source_id,sender_id,time DESC);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text,content='messages',content_rowid='rid',tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN INSERT INTO messages_fts(rowid,text) VALUES(new.rid,new.text); END;
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN INSERT INTO messages_fts(messages_fts,rowid,text) VALUES('delete',old.rid,old.text); END;
CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE OF text ON messages WHEN old.text IS NOT new.text BEGIN INSERT INTO messages_fts(messages_fts,rowid,text) VALUES('delete',old.rid,old.text); INSERT INTO messages_fts(rowid,text) VALUES(new.rid,new.text); END;
CREATE TABLE IF NOT EXISTS conversations(source_id TEXT NOT NULL,id TEXT NOT NULL,peer TEXT NOT NULL,kind TEXT NOT NULL,name TEXT NOT NULL,unread INTEGER NOT NULL DEFAULT 0,last_time INTEGER NOT NULL DEFAULT 0,last_text TEXT NOT NULL DEFAULT '',body TEXT NOT NULL DEFAULT '{}',message_count INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(source_id,id));
CREATE TABLE IF NOT EXISTS contacts(source_id TEXT NOT NULL,id TEXT NOT NULL,name TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(source_id,id));
CREATE TABLE IF NOT EXISTS members(source_id TEXT NOT NULL,conversation_id TEXT NOT NULL,id TEXT NOT NULL,name TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(source_id,conversation_id,id));
CREATE TABLE IF NOT EXISTS favorites(source_id TEXT NOT NULL,id TEXT NOT NULL,kind TEXT NOT NULL,time INTEGER NOT NULL,text TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(source_id,id));
CREATE TABLE IF NOT EXISTS checkpoints(source_id TEXT NOT NULL,conversation_id TEXT NOT NULL,time INTEGER NOT NULL,sequence INTEGER NOT NULL,id TEXT NOT NULL,PRIMARY KEY(source_id,conversation_id));
")?;
    let version: String =
        conn.query_row("SELECT value FROM settings WHERE key='schema'", [], |r| {
            r.get(0)
        })?;
    if version == "1" {
        conn.execute_batch("ALTER TABLE messages ADD COLUMN signature TEXT NOT NULL DEFAULT ''; UPDATE settings SET value='2' WHERE key='schema';")?;
    }
    if version == "1" || version == "2" {
        conn.execute_batch("BEGIN IMMEDIATE;
ALTER TABLE conversations ADD COLUMN message_count INTEGER NOT NULL DEFAULT 0;
UPDATE conversations SET message_count=(SELECT count(*) FROM messages m WHERE m.source_id=conversations.source_id AND m.conversation_id=conversations.id);
DROP TRIGGER messages_au;
CREATE TRIGGER messages_au AFTER UPDATE OF text ON messages WHEN old.text IS NOT new.text BEGIN INSERT INTO messages_fts(messages_fts,rowid,text) VALUES('delete',old.rid,old.text); INSERT INTO messages_fts(rowid,text) VALUES(new.rid,new.text); END;
UPDATE settings SET value='3' WHERE key='schema'; COMMIT;")?;
    }
    ensure!(
        ["1", "2", "3"].contains(&version.as_str()),
        "SCHEMA_UNSUPPORTED: 索引版本需要迁移"
    );
    conn.set_prepared_statement_cache_capacity(64);
    Ok(conn)
}
pub fn profiles(conn: &Connection, platform: &str) -> Result<Vec<Profile>> {
    let mut query = conn.prepare("SELECT body FROM profiles WHERE platform=? ORDER BY account")?;
    let rows = query.query_map([platform], |r| r.get::<_, String>(0))?;
    rows.map(|v| Ok(serde_json::from_str(&v?)?)).collect()
}
pub fn select(conn: &Connection, platform: &str, account: Option<&str>) -> Result<Profile> {
    let rows: Vec<_> = profiles(conn, platform)?
        .into_iter()
        .filter(|p| account.is_none_or(|a| p.account == a))
        .collect();
    ensure!(
        !rows.is_empty(),
        "NOT_INITIALIZED: 请先运行 {platform} init 或 import"
    );
    ensure!(
        rows.len() == 1,
        "ACCOUNT_AMBIGUOUS: 请使用 --account 明确选择账号"
    );
    Ok(rows.into_iter().next().unwrap())
}
pub fn save(conn: &Connection, profile: &Profile) -> Result<()> {
    conn.execute("INSERT INTO profiles VALUES(?,?,?,?) ON CONFLICT(source_id) DO UPDATE SET platform=excluded.platform,account=excluded.account,body=excluded.body",params![profile.source_id,profile.platform,profile.account,serde_json::to_string(profile)?])?;
    Ok(())
}
pub fn insert_message(
    conn: &Connection,
    p: &Profile,
    shard: &str,
    message: &Message,
    sequence: i64,
    signature: &str,
    legacy: bool,
) -> Result<()> {
    let body = serde_json::to_string(message)?;
    if legacy
        && conn
            .prepare_cached("UPDATE messages SET signature=? WHERE id=? AND body=?")?
            .execute(params![signature, message.id, body])?
            == 1
    {
        return Ok(());
    }
    conn.prepare_cached("INSERT INTO messages(id,source_id,shard,conversation_id,time,sequence,sender_id,kind,text,body,signature) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET shard=excluded.shard,conversation_id=excluded.conversation_id,time=excluded.time,sequence=excluded.sequence,sender_id=excluded.sender_id,kind=excluded.kind,text=excluded.text,body=excluded.body,signature=excluded.signature WHERE messages.signature<>excluded.signature OR messages.body<>excluded.body")?.execute(params![message.id,p.source_id,shard,message.conversation_id,message.time,sequence,message.sender_id,message.kind,message.text,body,signature])?;
    Ok(())
}
pub struct SyncSet {
    old: std::collections::HashMap<String, String>,
    force: bool,
}
impl SyncSet {
    pub fn new(conn: &Connection, p: &Profile, shard: &str, force: bool) -> Result<Self> {
        let mut st =
            conn.prepare("SELECT id,signature FROM messages WHERE source_id=? AND shard=?")?;
        let old = st
            .query_map(params![p.source_id, shard], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { old, force })
    }
    pub fn check(&mut self, id: &str, signature: &str) -> (bool, bool) {
        match self.old.remove(id) {
            Some(old) if !self.force && old == signature => (false, false),
            Some(old) => (true, old.is_empty()),
            None => (true, false),
        }
    }
    pub fn finish(self, conn: &Connection) -> Result<usize> {
        let n = self.old.len();
        let mut st = conn.prepare_cached("DELETE FROM messages WHERE id=?")?;
        for id in self.old.into_keys() {
            st.execute([id])?;
        }
        Ok(n)
    }
}
pub fn row_signature(parts: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"normalizer-v1");
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    hex::encode(hash.finalize())
}
pub fn conversation(
    conn: &Connection,
    p: &Profile,
    id: &str,
    peer: &str,
    kind: &str,
    name: &str,
) -> Result<()> {
    conn.execute("INSERT INTO conversations(source_id,id,peer,kind,name) VALUES(?,?,?,?,?) ON CONFLICT(source_id,id) DO UPDATE SET peer=excluded.peer,kind=excluded.kind,name=excluded.name",params![p.source_id,id,peer,kind,name])?;
    Ok(())
}
pub fn resolve(conn: &Connection, p: &Profile, selector: &str) -> Result<Value> {
    let mut st =
        conn.prepare("SELECT id,peer,kind,name,body FROM conversations WHERE source_id=?")?;
    let rows=st.query_map([&p.source_id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"peerId":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"name":r.get::<_,String>(3)?,"details":serde_json::from_str::<Value>(&r.get::<_,String>(4)?).unwrap_or(Value::Null)})))?;
    let mut exact = vec![];
    let mut matches = vec![];
    for row in rows {
        let row = row?;
        let id = row["id"].as_str().unwrap();
        let peer = row["peerId"].as_str().unwrap();
        let name = row["name"].as_str().unwrap();
        if selector == id || (selector == peer && (p.platform != "qq" || row["kind"] == "group")) {
            return Ok(row);
        }
        if selector.eq_ignore_ascii_case(name) || selector == peer {
            exact.push(row);
        } else if name.to_lowercase().contains(&selector.to_lowercase()) {
            matches.push(row);
        }
    }
    if !exact.is_empty() {
        matches = exact;
    }
    ensure!(
        !matches.is_empty(),
        "CONVERSATION_NOT_FOUND: 未找到会话 {selector}"
    );
    ensure!(
        matches.len() == 1,
        "CONVERSATION_AMBIGUOUS: 名称匹配多个会话，请使用完整 ID"
    );
    Ok(matches.remove(0))
}
pub fn finalize_conversations(conn: &Connection, p: &Profile) -> Result<()> {
    conn.execute("UPDATE conversations SET message_count=(SELECT count(*) FROM messages m WHERE m.source_id=conversations.source_id AND m.conversation_id=conversations.id) WHERE source_id=?",[&p.source_id])?;
    conn.execute("UPDATE conversations SET last_time=COALESCE((SELECT MAX(time) FROM messages WHERE source_id=conversations.source_id AND conversation_id=conversations.id),last_time),last_text=COALESCE((SELECT text FROM messages WHERE source_id=conversations.source_id AND conversation_id=conversations.id ORDER BY time DESC,sequence DESC,id DESC LIMIT 1),last_text) WHERE source_id=? AND COALESCE(json_extract(body,'$.unreadSource'),'')<>'client_snapshot'",[&p.source_id])?;
    Ok(())
}
pub fn metadata(p: &Profile) -> Value {
    json!({"platform":p.platform,"account":p.account,"sourceId":p.source_id,"syncedAt":p.synced_at,"freshness":"cached","capabilities":{"contacts":if p.platform=="qq"{"observed_correspondents"}else{"client_contacts"},"members":if p.platform=="qq"{"observed_senders"}else{"client_members"},"unread":if p.platform=="qq"{"cli_checkpoint"}else{"client_snapshot"}}})
}
pub fn profile_id(platform: &str, account: &str) -> String {
    crypto::digest(format!("{platform}\0{account}").as_bytes())[..24].into()
}
pub fn message_id(p: &Profile, original: &str) -> String {
    format!(
        "{}:{}:{original}",
        p.platform,
        p.account.replace('%', "%25").replace(':', "%3A")
    )
}

fn parse_time(value: &str, end: bool) -> Result<i64> {
    use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let dt = if end {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        }
        .unwrap();
        return Local
            .from_local_datetime(&dt)
            .single()
            .map(|v| v.timestamp())
            .ok_or_else(|| anyhow::anyhow!("INVALID_TIME: 日期处于时区切换区间"));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(value, fmt) {
            return Local
                .from_local_datetime(&dt)
                .single()
                .map(|v| v.timestamp())
                .ok_or_else(|| anyhow::anyhow!("INVALID_TIME"));
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(dt.timestamp());
    }
    bail!("INVALID_TIME: 日期格式无效")
}
pub struct Query {
    pub sql: String,
    pub values: Vec<rusqlite::types::Value>,
    pub limit: usize,
    pub offset: usize,
}
pub fn message_query(
    conn: &Connection,
    p: &Profile,
    args: &Value,
    chat: Option<&str>,
    search: Option<&str>,
) -> Result<Query> {
    use rusqlite::types::Value as S;
    let mut where_sql = vec!["m.source_id=?".to_string()];
    let mut values = vec![S::Text(p.source_id.clone())];
    let mut chats = vec![];
    if let Some(chat) = chat {
        chats.push(chat.to_string());
    }
    if let Some(v) = args.get("chat") {
        if let Some(v) = v.as_array() {
            for c in v {
                if let Some(c) = c.as_str() {
                    chats.push(c.into());
                }
            }
        } else if let Some(c) = v.as_str() {
            chats.push(c.into());
        }
    }
    if !chats.is_empty() {
        let mut placeholders = vec![];
        for chat in chats {
            let c = resolve(conn, p, &chat)?;
            values.push(S::Text(c["id"].as_str().unwrap().into()));
            placeholders.push("?");
        }
        where_sql.push(format!("m.conversation_id IN ({})", placeholders.join(",")));
    }
    let (mut start, mut end) = (
        string(args, "start-time")
            .map(|v| parse_time(v, false))
            .transpose()?,
        string(args, "end-time")
            .map(|v| parse_time(v, true))
            .transpose()?,
    );
    if args.get("days").is_some() {
        ensure!(
            start.is_none() && end.is_none(),
            "INVALID_ARGUMENT: --days 与时间范围不能同时使用"
        );
        let days = number(args, "days", 0);
        ensure!(
            days > 0 && days <= 36500,
            "INVALID_ARGUMENT: days 范围为 1..36500"
        );
        let now = chrono::Local::now();
        let date = now.date_naive() - chrono::Duration::days(days as i64 - 1);
        start = Some(parse_time(&date.to_string(), false)?);
        end = Some(now.timestamp());
    }
    ensure!(
        start.zip(end).is_none_or(|(a, b)| a <= b),
        "INVALID_ARGUMENT: 起始时间晚于结束时间"
    );
    if let Some(s) = start {
        where_sql.push("m.time>=?".into());
        values.push(S::Integer(s));
    }
    if let Some(e) = end {
        where_sql.push("m.time<=?".into());
        values.push(S::Integer(e));
    }
    if let Some(sender) = string(args, "sender") {
        where_sql.push("m.sender_id=?".into());
        values.push(S::Text(sender.into()));
    }
    if let Some(types) = args.get("type").and_then(Value::as_array) {
        let known = [
            "text", "image", "voice", "video", "sticker", "face", "location", "link", "file",
            "call", "card", "system", "mixed", "unknown",
        ];
        let mut clauses = vec![];
        for t in types {
            let t = t.as_str().unwrap_or("");
            ensure!(known.contains(&t), "INVALID_ARGUMENT: 消息类型无效");
            clauses.push("?");
            values.push(S::Text(t.into()));
        }
        if !clauses.is_empty() {
            where_sql.push(format!("m.kind IN ({})", clauses.join(",")));
        }
    }
    if let Some(keyword) = search {
        ensure!(!keyword.is_empty(), "INVALID_ARGUMENT: 关键词不能为空");
        if keyword.chars().count() >= 3 {
            where_sql.push(
                "m.rid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)".into(),
            );
            values.push(S::Text(format!("\"{}\"", keyword.replace('"', "\"\""))));
        }
        where_sql.push("instr(lower(m.text),lower(?))>0".into());
        values.push(S::Text(keyword.into()));
    }
    let ascending = match string(args, "order").unwrap_or("desc") {
        "asc" => true,
        "desc" => false,
        _ => bail!("INVALID_ARGUMENT: order 应为 asc 或 desc"),
    };
    let offset = number(args, "offset", 0);
    let limit = number(args, "limit", 50);
    ensure!(limit <= 1_000_000, "INVALID_ARGUMENT: limit 过大");
    if let Some(before) = string(args, "before") {
        ensure!(
            offset == 0,
            "INVALID_ARGUMENT: --before 与 --offset 不能同时使用"
        );
        let (time, seq, id) = if let Some((a, b)) = before.split_once(':') {
            (a.parse::<i64>()?, b.parse::<i64>()?, String::new())
        } else {
            let raw = hex::decode(before).map_err(|_| anyhow::anyhow!("INVALID_CURSOR"))?;
            let cursor: Value = serde_json::from_slice(&raw)?;
            (
                cursor[0]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("INVALID_CURSOR"))?,
                cursor[1]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("INVALID_CURSOR"))?,
                cursor[2]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("INVALID_CURSOR"))?
                    .to_owned(),
            )
        };
        where_sql.push("(m.time,m.sequence,m.id)<(?,?,?)".into());
        values.extend([S::Integer(time), S::Integer(seq), S::Text(id)]);
    }
    let order = if ascending { "ASC" } else { "DESC" };
    Ok(Query {
        sql: format!(
            "FROM messages m WHERE {} ORDER BY m.time {order},m.sequence {order},m.id {order}",
            where_sql.join(" AND ")
        ),
        values,
        limit,
        offset,
    })
}
pub fn messages(conn: &Connection, q: &Query) -> Result<Value> {
    let mut values = q.values.clone();
    values.extend([
        rusqlite::types::Value::Integer(q.limit as i64),
        rusqlite::types::Value::Integer(q.offset as i64),
    ]);
    let mut st = conn.prepare(&format!(
        "SELECT m.body,m.time,m.sequence,m.id {} LIMIT ? OFFSET ?",
        q.sql
    ))?;
    let mut rows = st.query(rusqlite::params_from_iter(values))?;
    let mut result = vec![];
    let mut cursor = Value::Null;
    while let Some(row) = rows.next()? {
        result.push(serde_json::from_str::<Value>(&row.get::<_, String>(0)?)?);
        cursor = json!([
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?
        ]);
    }
    Ok(
        json!({"count":result.len(),"messages":result,"nextCursor":if cursor.is_null(){Value::Null}else{Value::String(hex::encode(serde_json::to_vec(&cursor)?))}}),
    )
}
pub fn clean(conn: &mut Connection, home: &Path, p: &Profile, all: bool) -> Result<Value> {
    let tx = conn.transaction()?;
    for table in [
        "messages",
        "conversations",
        "contacts",
        "members",
        "favorites",
        "checkpoints",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE source_id=?"),
            [&p.source_id],
        )?;
    }
    if all {
        tx.execute("DELETE FROM profiles WHERE source_id=?", [&p.source_id])?;
    } else {
        let mut cleared = p.clone();
        cleared.files.clear();
        cleared.synced_at = None;
        save(&tx, &cleared)?;
    }
    tx.commit()?;
    let cache = home.join("cache").join(&p.source_id);
    if cache.is_dir() {
        let root = canonical(&home.join("cache"))?;
        let target = canonical(&cache)?;
        ensure!(
            target.starts_with(&root) && target != root,
            "INVALID_CACHE_PATH"
        );
        fs::remove_dir_all(target)?;
    }
    Ok(json!({"cleaned":true,"account":p.account,"configurationRemoved":all}))
}
