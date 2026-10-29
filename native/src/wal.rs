use crate::{crypto, model::WalState};
use anyhow::{Result, ensure};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

fn be(b: &[u8]) -> u32 {
    u32::from_be_bytes(b.try_into().unwrap())
}
fn checksum(bytes: &[u8], mut state: [u32; 2], little: bool) -> [u32; 2] {
    for chunk in bytes.chunks_exact(8) {
        let read = |c: &[u8]| {
            if little {
                u32::from_le_bytes(c.try_into().unwrap())
            } else {
                be(c)
            }
        };
        state[0] = state[0]
            .wrapping_add(read(&chunk[..4]))
            .wrapping_add(state[1]);
        state[1] = state[1]
            .wrapping_add(read(&chunk[4..]))
            .wrapping_add(state[0]);
    }
    state
}
pub struct Wal {
    pub state: WalState,
    pub frames: Vec<(u32, u64)>,
    pub incremental: bool,
}
pub fn inspect(path: &Path, previous: Option<&WalState>) -> Result<Wal> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    ensure!(size >= 32, "WAL_INVALID: WAL 头被截断");
    let mut hdr = [0; 32];
    file.read_exact(&mut hdr)?;
    let magic = be(&hdr[..4]);
    ensure!(
        magic == 0x377f0682 || magic == 0x377f0683,
        "WAL_INVALID: WAL 格式不匹配"
    );
    ensure!(be(&hdr[4..8]) == 3007000, "WAL_INVALID: WAL 版本不匹配");
    let page_size = be(&hdr[8..12]) as usize;
    ensure!(page_size == 4096, "WAL_INVALID: 微信页大小不匹配");
    let little = magic == 0x377f0682;
    let mut chain = checksum(&hdr[..24], [0, 0], little);
    ensure!(
        chain == [be(&hdr[24..28]), be(&hdr[28..32])],
        "WAL_INVALID: WAL 头校验失败"
    );
    let generation = crypto::digest(&hdr);
    let mut state = WalState {
        generation: generation.clone(),
        ..Default::default()
    };
    let mut frames = Vec::new();
    let mut frame = [0; 24];
    let mut data = vec![0; page_size];
    let mut count = 0;
    let mut prefix_ok = previous.is_some_and(|p| p.generation == generation && p.frames == 0);
    while file.stream_position()? + (24 + page_size) as u64 <= size {
        let offset = file.stream_position()?;
        file.read_exact(&mut frame)?;
        file.read_exact(&mut data)?;
        let pg = be(&frame[..4]);
        if pg == 0 || frame[8..16] != hdr[16..24] {
            break;
        }
        chain = checksum(&frame[..8], chain, little);
        chain = checksum(&data, chain, little);
        if chain != [be(&frame[16..20]), be(&frame[20..24])] {
            break;
        }
        count += 1;
        frames.push((pg, offset + 24));
        if let Some(p) = previous {
            if count == p.frames && p.generation == generation && chain == p.checksum {
                prefix_ok = true;
            }
        }
        let pages = be(&frame[4..8]);
        if pages != 0 {
            state.frames = count;
            state.checksum = chain;
            state.pages = pages;
        }
    }
    if let Some(previous) = previous {
        ensure!(
            previous.generation != generation || prefix_ok,
            "WAL_INVALID: 已提交的 WAL 前缀发生变化，缓存保留"
        );
    }
    frames.truncate(state.frames);
    let incremental = prefix_ok && previous.is_some_and(|p| p.frames <= state.frames);
    if incremental {
        let old = previous.unwrap().frames;
        frames.drain(..old);
    }
    Ok(Wal {
        state,
        frames,
        incremental,
    })
}
pub fn apply(
    path: &Path,
    output: &Path,
    wal: &Wal,
    key: Option<&[u8]>,
    mac_key: Option<&[u8]>,
) -> Result<usize> {
    let mut source = File::open(path)?;
    let mut dest = OpenOptions::new().read(true).write(true).open(output)?;
    let mut page = vec![0; 4096];
    for &(number, offset) in &wal.frames {
        source.seek(SeekFrom::Start(offset))?;
        source.read_exact(&mut page)?;
        let decoded = if let Some(key) = key {
            crypto::decrypt_wx_page(&page, number, key, mac_key.unwrap())?
        } else {
            page.clone()
        };
        dest.seek(SeekFrom::Start((number as u64 - 1) * 4096))?;
        dest.write_all(&decoded)?;
    }
    if wal.state.pages > 0 {
        dest.set_len(wal.state.pages as u64 * 4096)?;
    }
    dest.sync_all()?;
    Ok(wal.frames.len())
}
