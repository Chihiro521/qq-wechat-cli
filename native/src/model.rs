use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub protocol: u32,
    pub platform: String,
    pub command: String,
    pub data_dir: Option<PathBuf>,
    #[serde(default)]
    pub parent_pid: Option<u32>,
    #[serde(default)]
    pub args: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub platform: String,
    pub account: String,
    pub source: PathBuf,
    pub source_id: String,
    #[serde(default)]
    pub favorites: Option<PathBuf>,
    #[serde(default)]
    pub synced_at: Option<String>,
    #[serde(default)]
    pub files: BTreeMap<String, FileState>,
    #[serde(default)]
    pub vault: Option<String>,
    #[serde(default)]
    pub client: Option<ClientIdentity>,
    #[serde(default)]
    pub key_origin: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIdentity {
    pub pid: u32,
    pub created: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileState {
    pub fingerprint: String,
    pub db_fingerprint: String,
    pub cache: String,
    pub encrypted: bool,
    #[serde(default)]
    pub wal: Option<WalState>,
    #[serde(default)]
    pub pending_byte: Option<u32>,
    #[serde(default)]
    pub page_hashes: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WalState {
    pub generation: String,
    pub frames: usize,
    pub checksum: [u32; 2],
    pub pages: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub source_id: String,
    pub platform: String,
    pub account: String,
    pub conversation_id: String,
    pub conversation_name: String,
    pub conversation_kind: String,
    pub original_id: String,
    pub time: i64,
    pub sent_at: String,
    pub sender_id: String,
    pub sender_name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
    pub elements: Vec<Value>,
    pub decode_warnings: Vec<String>,
    #[serde(default)]
    pub metadata: Value,
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn stamp(time: i64) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|v| v.to_rfc3339())
        .unwrap_or_default()
}
pub fn string<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}
pub fn flag(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}
pub fn number(args: &Value, name: &str, default: usize) -> usize {
    args.get(name)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(default)
}
pub fn positional(args: &Value, index: usize) -> Option<&str> {
    args.get("positionals")?.as_array()?.get(index)?.as_str()
}
