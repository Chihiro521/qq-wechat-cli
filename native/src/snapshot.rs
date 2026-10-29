use crate::{crypto, windows};
use anyhow::{Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}
pub fn signature(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Ok("absent".into());
    }
    let m = fs::metadata(path)?;
    let stamp = m
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let mut file = File::open(path)?;
    let mut tail = vec![0; m.len().min(64) as usize];
    file.seek(SeekFrom::End(-(tail.len() as i64)))?;
    file.read_exact(&mut tail)?;
    Ok(format!(
        "{}:{stamp}:{}:{}",
        m.len(),
        crypto::digest(&crypto::header(path, 64)?),
        crypto::digest(&tail)
    ))
}
pub fn fingerprint(path: &Path) -> Result<(String, String)> {
    let db = signature(path)?;
    Ok((
        format!(
            "{db}|{}|{}",
            signature(&sidecar(path, "-wal"))?,
            if active_journal(path) {
                "active"
            } else {
                "inactive"
            }
        ),
        db,
    ))
}
fn active_journal(path: &Path) -> bool {
    crypto::header(&sidecar(path, "-journal"), 8)
        .ok()
        .is_some_and(|b| b == [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7])
}
pub fn copy_locked(source: &Path, target: &Path, calculate_hash: bool) -> Result<String> {
    let size = fs::metadata(source)?.len() as usize;
    if size == 0 {
        File::create(target)?;
        return Ok(crypto::digest(&[]));
    }
    // Read-only mapped views preserve bytes protected by SQLite's byte-range locks.
    // No lock-page bytes are zero-filled or skipped.
    let input = windows::MappedFile::open(source)?;
    let mut output = File::create(target)?;
    let mut buffer = vec![0; 1024 * 1024];
    let mut offset = 0;
    let mut hash = Sha256::new();
    while offset < size {
        let n = (size - offset).min(buffer.len());
        input.copy_into(offset, &mut buffer[..n])?;
        output.write_all(&buffer[..n])?;
        if calculate_hash {
            hash.update(&buffer[..n]);
        }
        offset += n;
    }
    Ok(if calculate_hash {
        hex::encode(hash.finalize())
    } else {
        String::new()
    })
}
pub struct Budget {
    remaining: Duration,
}
impl Budget {
    pub fn used_millis(&self) -> u128 {
        (Duration::from_secs(5) - self.remaining).as_millis()
    }
    pub fn new() -> Self {
        Self {
            remaining: Duration::from_secs(5),
        }
    }
}
pub struct Snapshot {
    pub db: PathBuf,
    pub wal: Option<PathBuf>,
    pub fingerprint: String,
    pub db_fingerprint: String,
}
pub fn capture(
    source: &Path,
    dir: &Path,
    pid: Option<u32>,
    budget: &mut Budget,
    header_only: bool,
) -> Result<Snapshot> {
    fs::create_dir_all(dir)?;
    let dest = dir.join("source.db");
    let wal_source = sidecar(source, "-wal");
    let wal_dest = dir.join("source.db-wal");
    let mut last = "".to_string();
    for _ in 0..3 {
        if active_journal(source) {
            std::thread::sleep(Duration::from_millis(100));
            last = "客户端事务仍在进行".into();
            continue;
        }
        let pause = if let Some(pid) = pid.filter(|_| budget.remaining > Duration::from_millis(10))
        {
            windows::Pause::acquire(pid, budget.remaining.as_millis() as u64).ok()
        } else {
            None
        };
        let started = pause
            .as_ref()
            .map(|p| p.started)
            .unwrap_or_else(Instant::now);
        let before = fingerprint(source)?;
        let result = (|| -> Result<Snapshot> {
            ensure!(
                !active_journal(source),
                "SNAPSHOT_UNSTABLE: QQ 正处于事务中"
            );
            let hash = if header_only {
                let bytes = crypto::header(source, 4096)?;
                fs::write(&dest, &bytes)?;
                crypto::digest(&bytes)
            } else {
                copy_locked(source, &dest, pause.is_none())?
            };
            let wal = if wal_source.is_file() && fs::metadata(&wal_source)?.len() >= 32 {
                copy_locked(&wal_source, &wal_dest, pause.is_none())?;
                Some(wal_dest.clone())
            } else {
                None
            };
            ensure!(
                before == fingerprint(source)? && !active_journal(source),
                "SNAPSHOT_UNSTABLE: 复制期间数据库发生变化（复制耗时 {} ms，暂停可用 {}）",
                started.elapsed().as_millis(),
                pause.is_some()
            );
            if pause.is_none() {
                let second = dir.join("verify.db");
                let second_hash = if header_only {
                    crypto::digest(&crypto::header(source, 4096)?)
                } else {
                    copy_locked(source, &second, true)?
                };
                let _ = fs::remove_file(second);
                ensure!(
                    hash == second_hash
                        && before == fingerprint(source)?
                        && !active_journal(source),
                    "SNAPSHOT_UNSTABLE: 两次副本不一致"
                );
            }
            Ok(Snapshot {
                db: dest.clone(),
                wal,
                fingerprint: before.0.clone(),
                db_fingerprint: before.1.clone(),
            })
        })();
        if pause.is_some() {
            budget.remaining = budget.remaining.saturating_sub(started.elapsed());
        }
        drop(pause);
        match result {
            Ok(s) => {
                fs::OpenOptions::new().write(true).open(&s.db)?.sync_all()?;
                if let Some(w) = &s.wal {
                    fs::OpenOptions::new().write(true).open(w)?.sync_all()?;
                }
                return Ok(s);
            }
            Err(error) => last = error.to_string(),
        };
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("SNAPSHOT_UNSTABLE: {last}；上次缓存保留")
}
pub fn normalize_header(path: &Path) -> Result<()> {
    let mut file = fs::OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(18))?;
    file.write_all(&[1, 1])?;
    file.sync_all()?;
    Ok(())
}
pub fn validate(path: &Path, full: bool) -> Result<()> {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.execute_batch("PRAGMA cache_size=-32768; PRAGMA mmap_size=134217728;")?;
    let pragma = if full {
        "PRAGMA integrity_check"
    } else {
        "PRAGMA quick_check"
    };
    let mut query = db.prepare(pragma)?;
    let mut rows = query.query([])?;
    let mut n = 0;
    while let Some(row) = rows.next()? {
        let result: String = row.get(0)?;
        ensure!(
            result == "ok",
            "DATABASE_INVALID: 解密副本未通过 SQLite 检查：{result}"
        );
        n += 1;
    }
    ensure!(n == 1, "DATABASE_INVALID");
    Ok(())
}
pub fn validate_qq(path: &Path, full: bool, known_pcqq: bool) -> Result<u32> {
    use crate::sqlite_compat::{PCQQ, PendingByte, STANDARD};
    if known_pcqq && fs::metadata(path)?.len() >= PCQQ as u64 {
        let _guard = PendingByte::new(PCQQ);
        validate(path, full)?;
        return Ok(PCQQ);
    }
    match validate(path, full) {
        Ok(()) => Ok(STANDARD),
        Err(original) => {
            if fs::metadata(path)?.len() >= PCQQ as u64 {
                let _guard = PendingByte::new(PCQQ);
                if validate(path, full).is_ok() {
                    return Ok(PCQQ);
                }
            }
            Err(original)
        }
    }
}
