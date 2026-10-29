use crate::{
    crypto,
    model::{FileState, Profile, Request, now, string},
    qq, snapshot, store, wal, wechat, windows,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Instant,
};
use zeroize::Zeroizing;

fn user_home() -> PathBuf {
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_else(|| ".".into()))
}
fn canonical(path: &Path) -> Result<PathBuf> {
    store::canonical(path).with_context(|| format!("SOURCE_NOT_FOUND: {}", path.display()))
}
pub fn legacy_path(platform: &str) -> PathBuf {
    if platform == "wechat" {
        std::env::var_os("WECHAT_CLI_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| user_home().join(".wechat-cli/config.json"))
    } else {
        std::env::var_os("QQ_CLI_CONFIG")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("QQ_CLI_HOME").map(|p| PathBuf::from(p).join("config.json"))
            })
            .unwrap_or_else(|| {
                let path = PathBuf::from(
                    std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| user_home().into()),
                )
                .join("qq-cli/config.json");
                let fallback = user_home().join("AppData/Local/qq-cli/config.json");
                if path.is_file() || !fallback.is_file() {
                    path
                } else {
                    fallback
                }
            })
    }
}
fn walk(root: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<()> {
    if depth == 0 {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let e = entry?;
        let kind = e.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(&e.path(), depth - 1, out)?;
        } else if kind.is_file()
            && e.path()
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("db"))
        {
            out.push(e.path());
        }
    }
    Ok(())
}
pub fn discover(platform: &str) -> Result<PathBuf> {
    let config = legacy_path(platform);
    if config.is_file() {
        let c: Value = serde_json::from_slice(&fs::read(config)?)?;
        if let Some(path) = c
            .get(if platform == "qq" {
                "sourceDb"
            } else {
                "db_dir"
            })
            .and_then(Value::as_str)
        {
            return canonical(Path::new(path));
        }
    }
    let roots = if platform == "qq" {
        vec![
            user_home().join("Documents/Tencent Files"),
            user_home().join("AppData/Roaming/Tencent/Users"),
        ]
    } else {
        vec![user_home().join("Documents/xwechat_files")]
    };
    let mut matches = vec![];
    for root in roots {
        if root.is_dir() {
            let mut paths = vec![];
            walk(&root, 4, &mut paths)?;
            for p in paths {
                if platform == "qq"
                    && p.file_name()
                        .is_some_and(|n| n.eq_ignore_ascii_case("Msg3.0.db"))
                {
                    matches.push(p);
                } else if platform == "wechat" && p.ends_with("contact/contact.db") {
                    if let Some(root) = p.parent().and_then(Path::parent) {
                        matches.push(root.into());
                    }
                }
            }
        }
    }
    matches.sort();
    matches.dedup();
    ensure!(
        matches.len() == 1,
        "SOURCE_AMBIGUOUS: 请用 --db（QQ）或 --db-dir（微信）指定数据库位置"
    );
    canonical(&matches.remove(0))
}
fn account_from_source(platform: &str, path: &Path) -> String {
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .unwrap_or_default()
        .to_string_lossy();
    if platform == "qq" && parent.chars().all(|c| c.is_ascii_digit()) && !parent.is_empty() {
        return parent.into();
    }
    if platform == "wechat" && !parent.is_empty() {
        return parent.into();
    }
    "default".into()
}
pub fn configure(request: &Request) -> Result<(Profile, Option<PathBuf>)> {
    let args = &request.args;
    let importing = request.command == "import";
    let mut legacy = Value::Null;
    let mut legacy_file = None;
    if importing {
        let file = string(args, "config")
            .map(PathBuf::from)
            .unwrap_or_else(|| legacy_path(&request.platform));
        ensure!(
            file.is_file(),
            "CONFIG_NOT_FOUND: 没有找到旧配置，可用 --config 指定"
        );
        legacy = serde_json::from_slice(&fs::read(&file)?)?;
        legacy_file = Some(file);
    }
    let source = if request.platform == "qq" {
        string(args, "db").or_else(|| legacy["sourceDb"].as_str())
    } else {
        string(args, "db-dir").or_else(|| legacy["db_dir"].as_str())
    };
    let source = if let Some(path) = source {
        canonical(Path::new(path))?
    } else {
        discover(&request.platform)?
    };
    ensure!(
        if request.platform == "qq" {
            source.is_file()
        } else {
            source.is_dir()
        },
        "INVALID_SOURCE: 数据源类型不匹配"
    );
    let account = string(args, "account")
        .or_else(|| string(args, "account-uin"))
        .map(str::to_owned)
        .or_else(|| legacy["accountUin"].as_str().map(str::to_owned))
        .unwrap_or_else(|| account_from_source(&request.platform, &source));
    ensure!(
        !account.is_empty() && account.len() <= 256,
        "INVALID_ACCOUNT"
    );
    let favorites = string(args, "favorites-db")
        .map(PathBuf::from)
        .or_else(|| legacy["favoritesDb"].as_str().map(PathBuf::from))
        .or_else(|| {
            if request.platform == "qq" {
                let path = source.parent()?.join("MyCollection/mc3.db");
                path.is_file().then_some(path)
            } else {
                None
            }
        });
    let keys_file = string(args, "keys-file")
        .map(PathBuf::from)
        .or_else(|| legacy["keys_file"].as_str().map(PathBuf::from))
        .or_else(|| {
            if importing && request.platform == "wechat" {
                Some(legacy_file?.parent()?.join("all_keys.json"))
            } else {
                None
            }
        });
    let key_origin = if string(args, "keys-file").is_some() {
        "provided"
    } else if keys_file.is_some() {
        "legacy"
    } else {
        "client"
    };
    let mut profile = Profile {
        source_id: store::profile_id(&request.platform, &account),
        platform: request.platform.clone(),
        account,
        source,
        favorites,
        synced_at: None,
        files: BTreeMap::new(),
        vault: None,
        client: None,
        key_origin: key_origin.into(),
    };
    if let Some(path) = keys_file.as_ref().filter(|p| p.is_file()) {
        let mut keys = BTreeMap::<String, String>::new();
        let secret = Zeroizing::new(fs::read(path)?);
        let raw: Value = serde_json::from_slice(&secret)?;
        if profile.platform == "qq" {
            if let Some(k) = raw["qq"].as_str() {
                ensure!(hex::decode(k)?.len() == 16, "KEY_MISMATCH");
                keys.insert("qq".into(), k.into());
            }
        } else if let Some(map) = raw.as_object() {
            for (name, value) in map {
                if name.starts_with('_') {
                    continue;
                }
                if let Some(key) = value.get("enc_key").and_then(Value::as_str) {
                    let salt = value
                        .get("salt")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| {
                            let relative = name.replace('\\', "/");
                            let rel = Path::new(&relative);
                            if rel.is_absolute()
                                || rel.components().any(|c| {
                                    matches!(
                                        c,
                                        std::path::Component::ParentDir
                                            | std::path::Component::Prefix(_)
                                    )
                                })
                            {
                                return None;
                            }
                            crypto::header(&profile.source.join(rel), 16)
                                .ok()
                                .map(hex::encode)
                        });
                    if let Some(salt) = salt {
                        if hex::decode(key)?.len() == 32 {
                            keys.insert(salt, key.into());
                        }
                    }
                }
            }
        }
        seal(&mut profile, &keys)?;
    }
    Ok((profile, keys_file))
}
pub fn seal(profile: &mut Profile, keys: &BTreeMap<String, String>) -> Result<()> {
    let bytes = Zeroizing::new(serde_json::to_vec(
        &json!({"platform":profile.platform,"account":profile.account,"source":profile.source,"keys":keys}),
    )?);
    profile.vault = Some(hex::encode(windows::protect(&bytes)?));
    Ok(())
}
pub fn unseal(profile: &Profile) -> Result<BTreeMap<String, String>> {
    let Some(vault) = &profile.vault else {
        return Ok(BTreeMap::new());
    };
    let secret = windows::unprotect(&hex::decode(vault)?)?;
    let raw: Value = serde_json::from_slice(&secret)?;
    ensure!(
        raw["platform"] == profile.platform
            && raw["account"] == profile.account
            && raw["source"] == json!(profile.source),
        "VAULT_IDENTITY_MISMATCH: 密钥缓存属于另一数据源"
    );
    Ok(serde_json::from_value(raw["keys"].clone())?)
}
fn files(profile: &Profile) -> Result<Vec<(String, PathBuf, bool)>> {
    if profile.platform == "qq" {
        let mut files = vec![("messages".into(), profile.source.clone(), false)];
        if let Some(fav) = &profile.favorites {
            if fav.is_file() {
                files.push(("favorites".into(), fav.clone(), true));
            }
        }
        return Ok(files);
    }
    let mut paths = vec![];
    walk(&profile.source, 4, &mut paths)?;
    let re = regex::Regex::new(r"^message/message_\d+\.db$")?;
    let mut out = vec![];
    for path in paths {
        let rel = path
            .strip_prefix(&profile.source)?
            .to_string_lossy()
            .replace('\\', "/");
        if [
            "contact/contact.db",
            "session/session.db",
            "favorite/favorite.db",
        ]
        .contains(&rel.as_str())
            || re.is_match(&rel)
        {
            out.push((rel, path, false));
        }
    }
    out.sort_by_key(|(name, _, _)| (!name.starts_with("contact/"), name.clone()));
    ensure!(
        !out.is_empty(),
        "DATABASE_SCHEMA_UNSUPPORTED: 没有可识别的微信数据库"
    );
    Ok(out)
}
pub fn refresh(
    conn: &mut Connection,
    home: &Path,
    mut profile: Profile,
    force: bool,
) -> Result<(Profile, Value)> {
    let started = Instant::now();
    conn.pragma_update(None, "cache_size", -32768)?;
    fs::create_dir_all(home.join("locks"))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(
            home.join("locks")
                .join(format!("{}.lock", profile.source_id)),
        )?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| anyhow::anyhow!("REFRESH_BUSY: 该账号正在刷新"))?;
    // Reload after taking the lock so a preceding process's successful refresh is reused.
    if !force {
        if let Ok(old) = store::select(conn, &profile.platform, Some(&profile.account)) {
            if old.source == profile.source
                && (profile.vault.is_none() || !profile.files.is_empty())
            {
                profile = old;
            }
        }
    }
    let sources = files(&profile)?;
    let cache_root = home.join("cache").join(&profile.source_id);
    fs::create_dir_all(&cache_root)?;
    let generation = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let stage = cache_root.join(generation);
    fs::create_dir(&stage)?;
    let result = (|| -> Result<(Profile, Value)> {
        let mut changes = vec![];
        let mut state = BTreeMap::new();
        for (name, path, favorite) in &sources {
            let (fingerprint, db_fingerprint) = snapshot::fingerprint(path)?;
            if !force {
                if let Some(previous) = profile.files.get(name) {
                    if previous.fingerprint == fingerprint && Path::new(&previous.cache).is_file() {
                        state.insert(name.clone(), previous.clone());
                        continue;
                    }
                }
            }
            changes.push((
                name.clone(),
                path.clone(),
                *favorite,
                fingerprint,
                db_fingerprint,
            ));
        }
        let removed: Vec<_> = profile
            .files
            .keys()
            .filter(|k| !sources.iter().any(|(name, _, _)| name == *k))
            .cloned()
            .collect();
        if changes.is_empty() && removed.is_empty() {
            return Ok((
                profile.clone(),
                json!({"changed":false,"changedFiles":0,"copiedBytes":0,"decryptedPages":0,"indexedMessages":0,"elapsedMs":started.elapsed().as_millis()}),
            ));
        }
        let mut keys = unseal(&profile)?;
        let mut pid = profile
            .client
            .as_ref()
            .filter(|c| windows::client_alive(c))
            .map(|c| c.pid);
        let mut wx_pages = BTreeMap::new();
        let mut wx_sample = BTreeMap::new();
        for (_, path, favorite, _, _) in &changes {
            if *favorite || crypto::plaintext(path)? {
                continue;
            }
            let head = crypto::header(path, 4096)?;
            if profile.platform == "qq" {
                ensure!(
                    head.starts_with(crypto::QQ_MAGIC),
                    "DATABASE_FORMAT_UNSUPPORTED: 不是支持的 Classic QQ 数据库"
                );
                let mut encrypted = crypto::header(path, 1024 + 8192)?;
                let valid = keys
                    .get("qq")
                    .and_then(|k| hex::decode(k).ok())
                    .is_some_and(|key| {
                        crypto::qq_block(&mut encrypted[1024..], &key, true).is_ok()
                            && encrypted[1024..].starts_with(crypto::SQLITE)
                    });
                if !valid {
                    ensure!(
                        profile.key_origin != "provided",
                        "KEY_MISMATCH: 提供的 QQ 密钥与数据库不匹配"
                    );
                    let (key, found_pid) = windows::recover_qq(path)?;
                    keys.insert("qq".into(), hex::encode(&*key));
                    pid = Some(found_pid);
                } else if pid.is_none() && profile.key_origin != "provided" {
                    if let Ok((_, found_pid)) = windows::recover_qq(path) {
                        pid = Some(found_pid);
                    }
                }
            } else {
                ensure!(head.len() == 4096, "DATABASE_TRUNCATED");
                let salt = hex::encode(&head[..16]);
                let valid = keys
                    .get(&salt)
                    .and_then(|k| hex::decode(k).ok())
                    .is_some_and(|key| crypto::verify_wx_key(&key, &head));
                if !valid {
                    ensure!(
                        profile.key_origin != "provided",
                        "KEY_MISMATCH: 提供的微信密钥与数据库不匹配"
                    );
                    wx_pages.insert(salt, head);
                } else if wx_sample.is_empty() {
                    wx_sample.insert(salt, head);
                }
            }
        }
        if !wx_pages.is_empty() {
            let (found, found_pid) = windows::recover_wechat(&wx_pages)?;
            keys.extend(found);
            pid = found_pid;
        } else if profile.platform == "wechat"
            && pid.is_none()
            && profile.key_origin != "provided"
            && !wx_sample.is_empty()
        {
            if let Ok((_, found_pid)) = windows::recover_wechat(&wx_sample) {
                pid = found_pid;
            }
        }
        profile.client = pid.and_then(windows::identity);
        let key_ms = started.elapsed().as_millis();
        let mut budget = snapshot::Budget::new();
        let mut prepared = vec![];
        let mut copied = 0u64;
        let mut cache_copied = 0u64;
        let mut decrypted = 0u64;
        let mut snapshot_ms = 0u128;
        let mut decrypt_ms = 0u128;
        let mut validation_ms = 0u128;
        for (name, path, favorite, _fingerprint, db_fingerprint) in &changes {
            eprintln!("正在刷新 {}：{}", profile.platform, name);
            let shard_dir = stage.join(&crypto::digest(name.as_bytes())[..16]);
            fs::create_dir(&shard_dir)?;
            let previous = profile.files.get(name);
            let header_only = profile.platform == "wechat"
                && !force
                && previous.is_some_and(|p| {
                    p.db_fingerprint == *db_fingerprint
                        && p.wal.is_some()
                        && Path::new(&p.cache).is_file()
                })
                && snapshot::sidecar(path, "-wal").is_file();
            let snapshot_started = Instant::now();
            let mut snap = snapshot::capture(path, &shard_dir, pid, &mut budget, header_only)?;
            snapshot_ms += snapshot_started.elapsed().as_millis();
            let decrypt_started = Instant::now();
            copied += fs::metadata(&snap.db)?.len();
            if let Some(w) = &snap.wal {
                copied += fs::metadata(w)?.len();
            }
            let target = shard_dir.join("plain.db");
            let encrypted = !crypto::plaintext(&snap.db)?;
            let mut wal_state = None;
            let mut page_hashes = None;
            if profile.platform == "qq" || *favorite {
                ensure!(
                    snap.wal.is_none(),
                    "DATABASE_MODE_UNSUPPORTED: 当前 QQ 格式不使用 WAL"
                );
                if encrypted {
                    ensure!(
                        !*favorite,
                        "FAVORITES_FORMAT_UNSUPPORTED: 当前 QQ 收藏库需要明文 SQLite"
                    );
                    let key = Zeroizing::new(hex::decode(
                        keys.get("qq")
                            .ok_or_else(|| anyhow::anyhow!("KEY_NOT_FOUND"))?,
                    )?);
                    let hashes = shard_dir.join("page-sha256.bin");
                    let previous = if force {
                        None
                    } else {
                        profile
                            .files
                            .get(name)
                            .and_then(|p| {
                                p.page_hashes
                                    .as_ref()
                                    .map(|h| (Path::new(&p.cache), Path::new(h)))
                            })
                            .filter(|(c, h)| c.is_file() && h.is_file())
                    };
                    if let Some((cache, _)) = previous {
                        cache_copied += fs::metadata(cache)?.len();
                    }
                    decrypted +=
                        crypto::decrypt_qq_delta(&snap.db, &target, &hashes, &key, previous)?;
                    page_hashes = Some(hashes.to_string_lossy().into());
                } else {
                    fs::rename(&snap.db, &target)?;
                }
            } else {
                let same_base = previous.is_some_and(|p| {
                    p.db_fingerprint == snap.db_fingerprint && Path::new(&p.cache).is_file()
                });
                let mut inspected = snap
                    .wal
                    .as_ref()
                    .map(|w| {
                        wal::inspect(
                            w,
                            if same_base {
                                previous.and_then(|p| p.wal.as_ref())
                            } else {
                                None
                            },
                        )
                    })
                    .transpose()?;
                let incremental =
                    !force && same_base && inspected.as_ref().is_some_and(|w| w.incremental);
                if header_only && !incremental {
                    snap = snapshot::capture(path, &shard_dir, pid, &mut budget, false)?;
                    copied += fs::metadata(&snap.db)?.len();
                    if let Some(w) = &snap.wal {
                        copied += fs::metadata(w)?.len();
                    }
                    inspected = snap
                        .wal
                        .as_ref()
                        .map(|w| wal::inspect(w, None))
                        .transpose()?;
                }
                let first = crypto::header(&snap.db, 4096)?;
                let key = if encrypted {
                    Some(Zeroizing::new(hex::decode(
                        keys.get(&hex::encode(&first[..16]))
                            .ok_or_else(|| anyhow::anyhow!("KEY_NOT_FOUND"))?,
                    )?))
                } else {
                    None
                };
                if incremental {
                    cache_copied += fs::copy(&previous.unwrap().cache, &target)?;
                } else if let Some(key) = &key {
                    decrypted += crypto::decrypt_wechat(&snap.db, &target, key)?;
                } else {
                    fs::rename(&snap.db, &target)?;
                }
                if let Some(mut wal) = inspected {
                    if !incremental {
                        wal = wal::inspect(snap.wal.as_ref().unwrap(), None)?;
                    }
                    let mac = key
                        .as_ref()
                        .map(|k| crypto::wx_mac_key(k, &first[..16]))
                        .transpose()?;
                    decrypted += wal::apply(
                        snap.wal.as_ref().unwrap(),
                        &target,
                        &wal,
                        key.as_ref().map(|k| k.as_slice()),
                        mac.as_ref().map(|m| m.as_slice()),
                    )? as u64;
                    wal_state = Some(wal.state);
                }
            }
            decrypt_ms += decrypt_started.elapsed().as_millis();
            let validation_started = Instant::now();
            snapshot::normalize_header(&target)?;
            let pending_byte = if profile.platform == "qq" && !*favorite {
                Some(snapshot::validate_qq(
                    &target,
                    profile.files.is_empty(),
                    encrypted,
                )?)
            } else {
                snapshot::validate(&target, profile.files.is_empty())?;
                None
            };
            validation_ms += validation_started.elapsed().as_millis();
            state.insert(
                name.clone(),
                FileState {
                    fingerprint: snap.fingerprint.clone(),
                    db_fingerprint: snap.db_fingerprint.clone(),
                    cache: target.to_string_lossy().into(),
                    encrypted,
                    wal: wal_state,
                    pending_byte,
                    page_hashes,
                },
            );
            prepared.push((name.clone(), target, *favorite, pending_byte));
            let _ = fs::remove_file(&snap.db);
            if let Some(w) = snap.wal {
                let _ = fs::remove_file(w);
            }
        }
        let decoded = started.elapsed().as_millis();
        let tx = conn.transaction()?;
        let changed_source = store::select(&tx, &profile.platform, Some(&profile.account))
            .ok()
            .is_some_and(|old| old.source != profile.source);
        if changed_source {
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
                    [&profile.source_id],
                )?;
            }
        }
        for name in &removed {
            tx.execute(
                "DELETE FROM messages WHERE source_id=? AND shard=?",
                params![profile.source_id, name],
            )?;
            if name == "favorites" || name.ends_with("favorite/favorite.db") {
                tx.execute(
                    "DELETE FROM favorites WHERE source_id=?",
                    [&profile.source_id],
                )?;
            }
            if name == "contact/contact.db" {
                tx.execute(
                    "DELETE FROM contacts WHERE source_id=?",
                    [&profile.source_id],
                )?;
                tx.execute(
                    "DELETE FROM members WHERE source_id=?",
                    [&profile.source_id],
                )?;
            }
            if name == "session/session.db" {
                tx.execute(
                    "DELETE FROM conversations WHERE source_id=?",
                    [&profile.source_id],
                )?;
            }
        }
        let mut indexed = 0;
        for (name, path, favorite, pending_byte) in prepared {
            if profile.platform == "qq" && !favorite {
                tx.execute(
                    "DELETE FROM conversations WHERE source_id=?",
                    [&profile.source_id],
                )?;
                tx.execute(
                    "DELETE FROM contacts WHERE source_id=?",
                    [&profile.source_id],
                )?;
            }
            indexed += if profile.platform == "qq" {
                if favorite {
                    qq::favorites(&tx, &profile, &path)?
                } else {
                    qq::import(
                        &tx,
                        &profile,
                        &path,
                        &name,
                        pending_byte.unwrap_or(crate::sqlite_compat::STANDARD),
                        force,
                    )?
                }
            } else {
                wechat::import(&tx, &profile, &path, &name, force)?
            };
        }
        store::finalize_conversations(&tx, &profile)?;
        profile.files = state;
        profile.synced_at = Some(now());
        seal(&mut profile, &keys)?;
        store::save(&tx, &profile)?;
        tx.commit()?;
        Ok((
            profile.clone(),
            json!({"changed":true,"changedFiles":changes.len(),"removedFiles":removed.len(),"copiedBytes":copied,"cacheCopiedBytes":cache_copied,"decryptedPages":decrypted,"indexedRecords":indexed,"keyMs":key_ms,"snapshotMs":snapshot_ms,"decryptMs":decrypt_ms,"validationMs":validation_ms,"pausedMs":budget.used_millis(),"snapshotAndDecryptMs":decoded,"indexMs":started.elapsed().as_millis()-decoded,"elapsedMs":started.elapsed().as_millis()}),
        ))
    })();
    // Only generated stage directories below this source's cache root are eligible for removal.
    if (result.is_err() && std::env::var_os("QQ_WECHAT_KEEP_FAILED_STAGE").is_none())
        || result.as_ref().is_ok_and(|(_, r)| r["changed"] == false)
    {
        let root = canonical(&cache_root)?;
        let target = canonical(&stage)?;
        ensure!(
            target.starts_with(&root) && target != root,
            "INVALID_CACHE_PATH"
        );
        let _ = fs::remove_dir_all(target);
    }
    if let Ok((profile, _)) = &result {
        let _ = collect_cache(home, profile);
    }
    drop(lock);
    result
}
fn collect_cache(home: &Path, profile: &Profile) -> Result<()> {
    let global = canonical(&home.join("cache"))?;
    let root = canonical(&global.join(&profile.source_id))?;
    ensure!(
        root.starts_with(&global) && root != global,
        "INVALID_CACHE_PATH"
    );
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
            continue;
        }
        let dir = canonical(&entry.path())?;
        ensure!(dir.starts_with(&root) && dir != root, "INVALID_CACHE_PATH");
        if !profile
            .files
            .values()
            .any(|f| Path::new(&f.cache).starts_with(&dir))
        {
            fs::remove_dir_all(dir)?;
        } else {
            // A generation can retain one unchanged shard; discard its superseded siblings.
            for shard in fs::read_dir(&dir)? {
                let shard = shard?;
                let name = shard.file_name().to_string_lossy().into_owned();
                if !shard.file_type()?.is_dir()
                    || shard.file_type()?.is_symlink()
                    || name.len() != 16
                    || !name.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    continue;
                }
                let path = canonical(&shard.path())?;
                ensure!(path.starts_with(&dir) && path != dir, "INVALID_CACHE_PATH");
                if !profile
                    .files
                    .values()
                    .any(|f| Path::new(&f.cache).starts_with(&path))
                {
                    fs::remove_dir_all(path)?;
                }
            }
        }
    }
    Ok(())
}
