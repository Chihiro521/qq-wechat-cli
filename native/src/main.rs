mod crypto;
mod model;
mod qq;
mod query;
mod snapshot;
mod source;
mod sqlite_compat;
mod store;
mod wal;
mod wechat;
mod windows;

use anyhow::{Result, bail, ensure};
use model::{Request, flag, string};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn result(data: Value, mut meta: Value) -> Result<()> {
    meta["peakMemoryBytes"] = json!(windows::peak_memory_bytes());
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(
        &mut out,
        &json!({"protocol":1,"kind":"result","ok":true,"data":data,"meta":meta}),
    )?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}
fn run(request: Request) -> Result<()> {
    if let Some(parent) = request.parent_pid {
        windows::watch_parent(parent)?;
    }
    ensure!(request.protocol == 1, "PROTOCOL_MISMATCH");
    ensure!(
        ["qq", "wechat"].contains(&request.platform.as_str()),
        "INVALID_PLATFORM"
    );
    let home = store::home(&request)?;
    let mut conn = store::open(&home)?;
    let account = string(&request.args, "account").or_else(|| string(&request.args, "account-uin"));
    if request.command == "status" {
        let profiles = store::profiles(&conn, &request.platform)?;
        let mut statuses = vec![];
        for profile in profiles
            .into_iter()
            .filter(|p| account.is_none_or(|a| p.account == a))
        {
            let messages: i64 = conn.query_row(
                "SELECT COALESCE(sum(message_count),0) FROM conversations WHERE source_id=?",
                [&profile.source_id],
                |r| r.get(0),
            )?;
            let conversations: i64 = conn.query_row(
                "SELECT count(*) FROM conversations WHERE source_id=?",
                [&profile.source_id],
                |r| r.get(0),
            )?;
            statuses.push(json!({"account":profile.account,"sourceId":profile.source_id,"source":profile.source,"syncedAt":profile.synced_at,"files":profile.files.len(),"messages":messages,"conversations":conversations,"keyCachePresent":profile.vault.is_some(),"capabilities":store::metadata(&profile)["capabilities"]}));
        }
        return result(
            json!({"initialized":!statuses.is_empty(),"platform":request.platform,"accounts":statuses,"indexBytes":std::fs::metadata(home.join("index.sqlite")).map(|m|m.len()).unwrap_or(0)}),
            json!({"mode":"cached"}),
        );
    }
    if request.command == "init" || request.command == "import" {
        let (profile, _) = source::configure(&request)?;
        let source = store::canonical(&profile.source)?;
        let own = store::canonical(&home)?;
        ensure!(
            !source.starts_with(own.join("cache")) && source != own.join("index.sqlite"),
            "INVALID_SOURCE: 不能把本工具的缓存设为源数据库"
        );
        if flag(&request.args, "dry-run") {
            return result(
                json!({"dryRun":true,"platform":profile.platform,"account":profile.account,"source":profile.source,"keyCachePresent":profile.vault.is_some()}),
                json!({}),
            );
        }
        let (profile, timing) =
            source::refresh(&mut conn, &home, profile, flag(&request.args, "force"))?;
        return result(
            json!({"initialized":true,"timing":timing}),
            store::metadata(&profile),
        );
    }
    let mut profile = store::select(&conn, &request.platform, account)?;
    if request.command == "refresh"
        && string(
            &request.args,
            if request.platform == "qq" {
                "db"
            } else {
                "db-dir"
            },
        )
        .is_some()
    {
        let mut reconfigure = Request {
            protocol: 1,
            platform: request.platform.clone(),
            command: "init".into(),
            data_dir: Some(home.clone()),
            args: request.args.clone(),
            parent_pid: None,
        };
        if reconfigure.args.get("account").is_none() {
            reconfigure.args["account"] = json!(profile.account);
        }
        profile = source::configure(&reconfigure)?.0;
    }
    if request.command == "refresh" || flag(&request.args, "refresh") {
        let (p, timing) = source::refresh(&mut conn, &home, profile, flag(&request.args, "force"))?;
        profile = p;
        if request.command == "refresh" {
            return result(timing, store::metadata(&profile));
        }
    }
    if request.command == "clean" {
        let lock = std::fs::OpenOptions::new()
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
        let data = store::clean(&mut conn, &home, &profile, flag(&request.args, "all"))?;
        drop(lock);
        return result(data, store::metadata(&profile));
    }
    ensure!(profile.synced_at.is_some(), "CACHE_EMPTY: 请先 refresh");
    if request.command == "export" {
        if let Some(output) = string(&request.args, "output") {
            let target = std::path::PathBuf::from(output);
            let target = if target.is_absolute() {
                target
            } else {
                std::env::current_dir()?.join(target)
            };
            let mut parent = target.as_path();
            while !parent.exists() {
                parent = parent
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("INVALID_OUTPUT"))?;
            }
            let ancestor = store::canonical(parent)?;
            let resolved = ancestor.join(target.strip_prefix(parent)?);
            let own = store::canonical(&home)?;
            let protected = if profile.source.is_dir() {
                ancestor.starts_with(&profile.source)
            } else {
                target == profile.source
                    || target.exists() && store::canonical(&target)? == profile.source
            } || profile
                .favorites
                .as_ref()
                .is_some_and(|p| store::canonical(p).ok().is_some_and(|p| p == resolved));
            ensure!(
                !protected
                    && !resolved.starts_with(own.join("cache"))
                    && !(resolved.parent() == Some(own.as_path())
                        && resolved
                            .file_name()
                            .is_some_and(|s| s.to_string_lossy().starts_with("index.sqlite"))),
                "OUTPUT_IS_SOURCE: 导出目标不能覆盖源数据库或查询索引"
            );
        }
        conn.execute_batch("BEGIN")?;
        query::export(&conn, &profile, &request.args)?;
        conn.execute_batch("COMMIT")?;
        return Ok(());
    }
    let data = match request.command.as_str() {
        "sessions" => query::sessions(&conn, &profile, &request.args, false)?,
        "unread" => query::sessions(&conn, &profile, &request.args, true)?,
        "contacts" => query::contacts(&conn, &profile, &request.args)?,
        "members" => query::members(&conn, &profile, &request.args)?,
        "favorites" => query::favorites(&conn, &profile, &request.args)?,
        "history" => query::history(&conn, &profile, &request.args, false)?,
        "search" => query::history(&conn, &profile, &request.args, true)?,
        "stats" => query::statistics(&conn, &profile, &request.args)?,
        "summarize" => query::summarize(&conn, &profile, &request.args)?,
        "new-messages" => query::new_messages(&mut conn, &profile, &request.args)?,
        _ => bail!("UNKNOWN_COMMAND: 未知命令"),
    };
    result(data, store::metadata(&profile))
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|x| x == "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.get(1).is_some_and(|x| x == "--pause-watchdog") {
        if windows::watchdog(&args[2..]).is_err() {
            std::process::exit(1);
        }
        return;
    }
    for line in std::io::stdin().lock().lines() {
        let outcome = (|| -> Result<()> {
            let line = line?;
            ensure!(line.len() <= 1024 * 1024, "REQUEST_TOO_LARGE");
            let request: Request = serde_json::from_str(&line)?;
            run(request)
        })();
        if let Err(error) = outcome {
            let text = error.to_string();
            let (prefix, remainder) = text.split_once(": ").unwrap_or(("ENGINE_ERROR", &text));
            let (code, message) = if prefix.bytes().all(|c| c.is_ascii_uppercase() || c == b'_') {
                (prefix, remainder)
            } else {
                ("ENGINE_ERROR", text.as_str())
            };
            println!(
                "{}",
                json!({"protocol":1,"kind":"result","ok":false,"error":{"code":code,"message":message}})
            );
            std::process::exit(1);
        }
    }
}
