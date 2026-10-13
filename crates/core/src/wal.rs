//! SQLite WAL 解析.
//!
//! 撤回会删除消息行, 而被删行所在页的**旧版本**只留在 WAL 里被取代的帧中, SQLite 的接口
//! 看不到它们。要还原内容, 就得自己按帧读 WAL。
//!
//! 文件布局 (与 SQLite 的 `walDecodeFrame` 一致):
//!
//! ```text
//! 头 32 字节: magic(4) 版本(4) 页大小(4) 检查点序号(4) 盐值(8) 校验和(8)
//! 每帧:       页号(4) 库页数(4) 盐值(8) 校验和(8) 页数据(页大小)
//! ```
//!
//! 两个容易踩的点:
//!
//! - 库页数**非 0 表示这是提交帧**, 即该事务的最后一帧
//! - WAL 是固定大小的环形文件。文件里往往同时躺着**好几代**帧: 当前代 (盐值与文件头一致) 与其
//!   它已经被检查点冲刷过的旧代。旧代的文件头早已被覆盖, 但其帧字节仍在原处, 它们正是找回
//!   被删内容的去处。旧代的第一帧无法验证校验和 (链的起点是已消失的旧文件头), 只能信任帧里
//!   记录的校验和, 之后各帧照常逐帧验证

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{CoreError, Result};

/// WAL 头长度.
pub const WAL_HEADER_SIZE: usize = 32;

/// 帧头长度.
pub const FRAME_HEADER_SIZE: usize = 24;

/// 小端校验和的 magic; 低字节置位表示大端校验和.
const WAL_MAGIC: u32 = 0x377f_0682;

/// 支持的 WAL 版本.
const WAL_VERSION: u32 = 3_007_000;

/// WAL 解析错误.
#[derive(Debug, thiserror::Error)]
pub enum WalError {
    /// 文件头都不完整.
    #[error("不足 {WAL_HEADER_SIZE} 字节, 不像是 WAL 文件")]
    TooShort,

    /// magic 不对.
    #[error("magic 不是 WAL: 0x{0:08x}")]
    BadMagic(u32),

    /// 页大小不合法.
    #[error("页大小不合法: {0}")]
    BadPageSize(u32),

    /// 版本不支持.
    #[error("不支持的 WAL 版本: {0}")]
    BadVersion(u32),

    /// WAL 头的校验和对不上.
    #[error("WAL 头的校验和对不上")]
    BadHeaderChecksum,

    /// 指定的提交帧超范围.
    #[error("提交帧序号 {index} 超出范围, 该 WAL 共有 {count} 个提交帧")]
    CommitOutOfRange {
        /// 请求的序号 (0 起).
        index: usize,
        /// 实际提交帧数量.
        count: usize,
    },
}

/// WAL 头.
#[derive(Debug, Clone)]
pub struct WalHeader {
    /// 页大小.
    pub page_size: u32,
    /// 检查点序号.
    pub checkpoint_sequence: u32,
    /// 盐值, 当前代的帧头盐值必须与它一致.
    pub salt: [u8; 8],
    /// 校验和是否为大端序 (由 magic 的最低位决定).
    pub big_endian_checksum: bool,
}

/// 一帧.
#[derive(Debug, Clone)]
pub struct Frame {
    /// 帧序号, 从 1 开始, 指它在文件里的位置.
    pub index: u32,
    /// 页号.
    pub page_number: u32,
    /// 该事务提交后库的页数; 为 0 表示这一帧不是提交帧.
    pub db_size: u32,
    /// 页数据 (未解密).
    pub page: Vec<u8>,
}

impl Frame {
    /// 是否为提交帧 (事务的最后一帧).
    pub fn is_commit(&self) -> bool {
        self.db_size != 0
    }
}

/// 一代 WAL: 盐值相同的一段连续帧.
#[derive(Debug, Clone)]
pub struct Generation {
    /// 这一代的盐值.
    pub salt: [u8; 8],
    /// 是否就是文件头所指的那一代.
    pub current: bool,
    /// 第一帧的校验和是否真的验证过.
    ///
    /// 只有当前代能接着文件头的校验和验证; 旧代的文件头已被覆盖, 其第一帧只能信任帧里记录的
    /// 值, 因此为 `false`。
    pub head_verified: bool,
    /// 帧, 按帧序.
    pub frames: Vec<Frame>,
}

impl Generation {
    /// 提交帧在本代帧列表里的下标 (0 起).
    pub fn commits(&self) -> Vec<usize> {
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.is_commit())
            .map(|(offset, _)| offset)
            .collect()
    }

    /// 本代全部提交后库应有的页数.
    pub fn committed_page_count(&self) -> Option<u32> {
        self.frames
            .iter()
            .rev()
            .find(|frame| frame.is_commit())
            .map(|frame| frame.db_size)
    }

    /// 按页号分组, 保留该页在本代里出现的所有帧 (按帧序).
    pub fn versions_by_page(&self) -> BTreeMap<u32, Vec<usize>> {
        let mut versions: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (offset, frame) in self.frames.iter().enumerate() {
            versions.entry(frame.page_number).or_default().push(offset);
        }
        versions
    }
}

/// 解析好的 WAL.
#[derive(Debug, Clone)]
pub struct Wal {
    /// 文件头.
    pub header: WalHeader,
    /// 按文件位置排列的各代帧.
    pub generations: Vec<Generation>,
    /// 文件大小.
    pub file_size: u64,
    /// 未能归入任何一代的字节数.
    pub unparsed_bytes: usize,
}

impl Wal {
    /// 从字节流解析, 便于测试.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < WAL_HEADER_SIZE {
            return Err(WalError::TooShort.into());
        }

        let magic = read_u32(data, 0);
        if magic & 0xffff_fffe != WAL_MAGIC {
            return Err(WalError::BadMagic(magic).into());
        }
        let big_endian_checksum = magic & 1 == 1;
        // 本机字节序与校验和字节序一致时按本机字读取, 否则逐字交换。
        let swap = big_endian_checksum != cfg!(target_endian = "big");

        let version = read_u32(data, 4);
        if version != WAL_VERSION {
            return Err(WalError::BadVersion(version).into());
        }
        let page_size = read_u32(data, 8);
        if page_size < 512 || !page_size.is_power_of_two() {
            return Err(WalError::BadPageSize(page_size).into());
        }

        let header_checksum = checksum(&data[..24], (0, 0), swap);
        if header_checksum != (read_u32(data, 24), read_u32(data, 28)) {
            return Err(WalError::BadHeaderChecksum.into());
        }

        let mut salt = [0u8; 8];
        salt.copy_from_slice(&data[16..24]);
        let header = WalHeader {
            page_size,
            checkpoint_sequence: read_u32(data, 12),
            salt,
            big_endian_checksum,
        };

        let stride = FRAME_HEADER_SIZE + page_size as usize;
        let available = data.len().saturating_sub(WAL_HEADER_SIZE);
        let candidates = available / stride;

        let mut generations = Vec::new();
        let mut parsed_frames = 0usize;
        let mut index = 1usize;
        while index <= candidates {
            let first = frame_slice(data, index, stride);
            let generation_salt = first[8..16].to_vec();
            let page_number = read_u32(first, 0);
            if page_number == 0 {
                break;
            }

            // 盐值与文件头一致的这一代就是当前正在使用的那一代。
            //
            // 只有紧跟在文件头之后的那一代, 其第一帧能接着文件头校验和验证; 其余的 (包括
            // 文件头被覆盖的旧代, 以及文件头之后位置不连续的情况) 只能信任帧里记录的校验和,
            // 之后各帧照常逐帧验证。
            let current = generation_salt == header.salt;
            let head_verified = current && index == 1;
            let stored = (read_u32(first, 16), read_u32(first, 20));
            let mut running = if head_verified {
                header_checksum
            } else {
                stored
            };
            if head_verified {
                // 首帧的校验和以文件头为起点算, 算完就把链条推进到帧自己的值。
                let Some(next) = frame_checksum(first, header_checksum, swap) else {
                    break;
                };
                if next != stored {
                    // 当前代第一帧就坏了: 整份 WAL 视为到此为止。
                    break;
                }
                running = next;
            }

            let mut frames = vec![to_frame(first, index)];
            let mut next_index = index + 1;
            while next_index <= candidates {
                let candidate = frame_slice(data, next_index, stride);
                if candidate[8..16] != generation_salt {
                    break;
                }
                if read_u32(candidate, 0) == 0 {
                    break;
                }
                let Some(next) = frame_checksum(candidate, running, swap) else {
                    break;
                };
                if next != (read_u32(candidate, 16), read_u32(candidate, 20)) {
                    break;
                }
                running = next;
                frames.push(to_frame(candidate, next_index));
                next_index += 1;
            }

            // 只认有说服力的一代: 至少两帧, 或首帧能验证, 或就是当前代, 或首帧即提交帧。
            let keep = frames.len() >= 2 || head_verified || current || frames[0].is_commit();
            if keep {
                parsed_frames += frames.len();
                generations.push(Generation {
                    salt: generation_salt.try_into().expect("8 字节盐值"),
                    current,
                    head_verified,
                    frames,
                });
            }
            index = next_index;
        }

        let unparsed_bytes = available.saturating_sub(parsed_frames * stride);
        Ok(Self {
            header,
            generations,
            file_size: data.len() as u64,
            unparsed_bytes,
        })
    }

    /// 读取 WAL 文件.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = std::fs::read(path).map_err(|source| CoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&data)
    }

    /// 读取 WAL 文件; 文件不存在时返回 `None`.
    ///
    /// 库没有 WAL 是正常情况 (已经检查点过), 因此这里不当作错误。
    pub fn open_if_present(path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }
        Self::open(path).map(Some)
    }

    /// 文件头所指的那一代.
    pub fn current(&self) -> Option<&Generation> {
        self.generations
            .iter()
            .find(|generation| generation.current)
    }

    /// 帧总数 (含旧代).
    pub fn len(&self) -> usize {
        self.generations
            .iter()
            .map(|generation| generation.frames.len())
            .sum()
    }

    /// 是否没有任何有效帧.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 遍历所有代的帧, 附带它属于第几代.
    pub fn all_frames(&self) -> impl Iterator<Item = (usize, &Frame)> {
        self.generations
            .iter()
            .enumerate()
            .flat_map(|(index, generation)| {
                generation.frames.iter().map(move |frame| (index, frame))
            })
    }

    /// 当前代的提交帧下标 (0 起).
    pub fn commits(&self) -> Vec<usize> {
        self.current().map(Generation::commits).unwrap_or_default()
    }

    /// 当前代全部提交后库应有的页数.
    pub fn committed_page_count(&self) -> Option<u32> {
        self.current().and_then(Generation::committed_page_count)
    }
}

/// 取第 `index` 帧在文件里的切片 (index 从 1 开始).
fn frame_slice(data: &[u8], index: usize, stride: usize) -> &[u8] {
    let start = WAL_HEADER_SIZE + (index - 1) * stride;
    &data[start..start + stride]
}

/// 按前驱校验和算出一帧的校验和.
fn frame_checksum(frame: &[u8], running: (u32, u32), swap: bool) -> Option<(u32, u32)> {
    if frame.len() < FRAME_HEADER_SIZE {
        return None;
    }
    let (s1, s2) = checksum(&frame[..8], running, swap);
    Some(checksum(&frame[FRAME_HEADER_SIZE..], (s1, s2), swap))
}

/// 把帧字节转成 [`Frame`].
fn to_frame(frame: &[u8], index: usize) -> Frame {
    Frame {
        index: index as u32,
        page_number: read_u32(frame, 0),
        db_size: read_u32(frame, 4),
        page: frame[FRAME_HEADER_SIZE..].to_vec(),
    }
}

/// 读 4 字节大端整数.
fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// SQLite 的 WAL 校验和.
///
/// 逐个 4 字节字交替累加: `s1 = s1 + 字 + s2`, 下一个字则累加到 `s2`, 如此交替。
/// `swap` 为真表示文件里的校验和是大端序, 需要逐字交换。
fn checksum(data: &[u8], init: (u32, u32), swap: bool) -> (u32, u32) {
    let (mut s1, mut s2) = init;
    for (index, chunk) in data.as_chunks::<4>().0.iter().enumerate() {
        let word = u32::from_ne_bytes(*chunk);
        let word = if swap { word.swap_bytes() } else { word };
        if index % 2 == 0 {
            s1 = s1.wrapping_add(word).wrapping_add(s2);
        } else {
            s2 = s2.wrapping_add(word).wrapping_add(s1);
        }
    }
    (s1, s2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个两帧的 WAL: 第一帧写页 2, 第二帧写页 3 并提交.
    fn sample_wal() -> Vec<u8> {
        let page_size = 512u32;
        let salt = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let swap = false;
        let mut data = build_header(page_size, salt, swap);
        let header_checksum = checksum(&data[..24], (0, 0), swap);

        let (frame_a, running) = build_frame(
            2,
            0,
            salt,
            &vec![0xaa; page_size as usize],
            header_checksum,
            swap,
        );
        data.extend_from_slice(&frame_a);
        let (frame_b, _) = build_frame(3, 7, salt, &vec![0xbb; page_size as usize], running, swap);
        data.extend_from_slice(&frame_b);
        data
    }

    /// 造一个旧代: 盐值与文件头不同.
    fn stale_generation(page_size: u32, salt: [u8; 8], pages: &[u32]) -> Vec<u8> {
        let swap = false;
        let mut running = (0u32, 0u32);
        let mut out = Vec::new();
        for (index, page) in pages.iter().enumerate() {
            let commit = if index + 1 == pages.len() { 20 } else { 0 };
            // 旧代第一帧的校验和是"信任值", 这里用真实计算值即可。
            let (frame, next) = build_frame(
                *page,
                commit,
                salt,
                &vec![index as u8; page_size as usize],
                running,
                swap,
            );
            running = next;
            let _ = index;
            out.extend_from_slice(&frame);
        }
        out
    }

    #[test]
    fn parses_current_generation() {
        let data = sample_wal();
        let wal = Wal::parse(&data).expect("应能解析");
        assert_eq!(wal.generations.len(), 1);
        let current = wal.current().expect("应有当前代");
        assert!(current.current);
        assert!(current.head_verified);
        assert_eq!(current.frames.len(), 2);
        assert!(!current.frames[0].is_commit());
        assert!(current.frames[1].is_commit());
        assert_eq!(current.committed_page_count(), Some(7));
        assert_eq!(wal.commits(), vec![1]);
    }

    #[test]
    fn keeps_stale_generation_after_header_rewrite() {
        // 旧代在前, 之后文件头盐值变化: 旧代仍应被解析出来。
        let page_size = 512u32;
        let stale_salt = [9u8; 8];
        let mut data = build_header(page_size, [1u8; 8], false);
        data.extend_from_slice(&stale_generation(page_size, stale_salt, &[4, 5, 6]));
        // 追加一个属于当前代的帧 (盐值与头一致), 放在旧代之后。
        let header_checksum = checksum(&data[..24], (0, 0), false);
        let (frame, _) = build_frame(
            2,
            0,
            [1u8; 8],
            &vec![0x11; page_size as usize],
            header_checksum,
            false,
        );
        // 注意: 这里为构造方便直接把帧追加在末尾, 位置不影响解析逻辑。
        data.extend_from_slice(&frame);

        let wal = Wal::parse(&data).expect("应能解析");
        assert_eq!(wal.generations.len(), 2, "旧代与后续帧都应当保留");
        assert!(wal.generations[0].frames.len() >= 2);
        assert_eq!(wal.generations[0].salt, stale_salt);
        assert!(!wal.generations[0].current);
    }

    #[test]
    fn stops_at_stale_tail() {
        let mut data = sample_wal();
        let mut junk = vec![0u8; FRAME_HEADER_SIZE + 512];
        junk[0..4].copy_from_slice(&9u32.to_be_bytes());
        junk[8..16].copy_from_slice(&[9u8; 8]);
        data.extend_from_slice(&junk);

        let wal = Wal::parse(&data).expect("应能解析");
        assert_eq!(wal.len(), 2, "垃圾帧不应被计入");
        assert!(wal.unparsed_bytes > 0);
    }

    #[test]
    fn rejects_bad_header() {
        assert!(Wal::parse(&[0u8; 8]).is_err());
        let mut data = build_header(512, [0; 8], false);
        data[24] ^= 0xff;
        assert!(matches!(
            Wal::parse(&data),
            Err(CoreError::Wal(WalError::BadHeaderChecksum))
        ));
    }

    /// 造文件头.
    fn build_header(page_size: u32, salt: [u8; 8], swap: bool) -> Vec<u8> {
        let mut header = vec![0u8; WAL_HEADER_SIZE];
        header[0..4].copy_from_slice(&WAL_MAGIC.to_be_bytes());
        header[4..8].copy_from_slice(&WAL_VERSION.to_be_bytes());
        header[8..12].copy_from_slice(&page_size.to_be_bytes());
        header[12..16].copy_from_slice(&0u32.to_be_bytes());
        header[16..24].copy_from_slice(&salt);
        let (s1, s2) = checksum(&header[..24], (0, 0), swap);
        header[24..28].copy_from_slice(&s1.to_be_bytes());
        header[28..32].copy_from_slice(&s2.to_be_bytes());
        header
    }

    /// 造一帧, 返回 (字节, 该帧的校验和).
    fn build_frame(
        page_number: u32,
        db_size: u32,
        salt: [u8; 8],
        page: &[u8],
        running: (u32, u32),
        swap: bool,
    ) -> (Vec<u8>, (u32, u32)) {
        let mut frame = vec![0u8; FRAME_HEADER_SIZE + page.len()];
        frame[0..4].copy_from_slice(&page_number.to_be_bytes());
        frame[4..8].copy_from_slice(&db_size.to_be_bytes());
        frame[8..16].copy_from_slice(&salt);
        frame[FRAME_HEADER_SIZE..].copy_from_slice(page);

        let next = frame_checksum(&frame, running, swap).expect("帧足够长");
        frame[16..20].copy_from_slice(&next.0.to_be_bytes());
        frame[20..24].copy_from_slice(&next.1.to_be_bytes());
        (frame, next)
    }
}
