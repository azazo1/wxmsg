//! 把加密库重建为明文副本, 以及按页读取.
//!
//! SQLCipher 的明文视图与磁盘文件只差页内布局: 每页去掉保留区, 页 1 另有 16 字节盐值。
//! 重建时把每页解出的明文补齐到页大小即可, SQLite 会依据文件头第 20 字节的保留区长度
//! 正确处理末尾的空白。
//!
//! 明文副本包含完整聊天记录, 只应在需要排错或与 SQLCipher 视图对照时生成, 用完删除。

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::crypto::{CipherParams, KEY_SIZE, PageCipher};
use crate::error::{CoreError, Result};

/// 重建结果.
#[derive(Debug, Clone, Copy)]
pub struct RebuildReport {
    /// 已解密的页数.
    pub pages: usize,
    /// 本次使用的参数.
    pub params: CipherParams,
}

/// 逐页解密, 生成明文的 SQLite 库.
pub fn rebuild_plaintext(
    source: impl AsRef<Path>,
    key: [u8; KEY_SIZE],
    params: CipherParams,
    target: impl AsRef<Path>,
) -> Result<RebuildReport> {
    let source = source.as_ref();
    let target = target.as_ref();
    let cipher = PageCipher::from_db_file(source, key, params)?;
    let page_size = params.page_size();

    let input = File::open(source).map_err(|error| CoreError::Io {
        path: source.to_path_buf(),
        source: error,
    })?;
    let mut input = BufReader::new(input);
    let output = File::create(target).map_err(|error| CoreError::Io {
        path: target.to_path_buf(),
        source: error,
    })?;
    let mut output = BufWriter::new(output);

    let mut buffer = vec![0u8; page_size];
    let mut pages = 0usize;
    loop {
        let read = read_full(&mut input, &mut buffer, source)?;
        if read == 0 {
            break;
        }
        if read != page_size {
            return Err(CoreError::PageSize {
                page: pages as u64 + 1,
                expected: page_size,
                actual: read,
            });
        }
        pages += 1;
        let page = cipher.decrypt_page(pages as u64, &buffer)?;
        output.write_all(&page).map_err(|error| CoreError::Io {
            path: target.to_path_buf(),
            source: error,
        })?;
    }

    output.flush().map_err(|error| CoreError::Io {
        path: target.to_path_buf(),
        source: error,
    })?;
    Ok(RebuildReport { pages, params })
}

/// 读取并解密指定的页 (页号从 1 开始).
pub fn read_page(
    source: impl AsRef<Path>,
    cipher: &PageCipher,
    page_number: u64,
) -> Result<Vec<u8>> {
    let reader = PageReader::open(source, cipher.clone())?;
    reader.read(page_number)
}

/// 反复读取同一个库里的页.
///
/// 每次调用都开一次文件太浪费, 这里把文件句柄留着; 内部用 `RefCell` 共享游标,
/// 因此本类型不是 `Sync`, 单线程遍历足够。
pub struct PageReader {
    file: std::cell::RefCell<File>,
    cipher: PageCipher,
    path: std::path::PathBuf,
}

impl PageReader {
    /// 打开库.
    pub fn open(source: impl AsRef<Path>, cipher: PageCipher) -> Result<Self> {
        let path = source.as_ref().to_path_buf();
        let file = File::open(&path).map_err(|error| CoreError::Io {
            path: path.clone(),
            source: error,
        })?;
        Ok(Self {
            file: std::cell::RefCell::new(file),
            cipher,
            path,
        })
    }

    /// 读并解密一页.
    pub fn read(&self, page_number: u64) -> Result<Vec<u8>> {
        let page_size = self.cipher.params().page_size();
        let mut buffer = vec![0u8; page_size];
        {
            let mut file = self.file.borrow_mut();
            crate::io::retry(|| file.seek(SeekFrom::Start((page_number - 1) * page_size as u64)))
                .map_err(|error| CoreError::Io {
                path: self.path.clone(),
                source: error,
            })?;
            let mut filled = 0;
            while filled < page_size {
                let read =
                    crate::io::retry(|| file.read(&mut buffer[filled..])).map_err(|error| {
                        CoreError::Io {
                            path: self.path.clone(),
                            source: error,
                        }
                    })?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            if filled != page_size {
                return Err(CoreError::PageSize {
                    page: page_number,
                    expected: page_size,
                    actual: filled,
                });
            }
        }
        self.cipher.decrypt_page(page_number, &buffer)
    }
}

/// 文件包含的页数 (向上取整, 不足一页时也算一页).
pub fn page_count(source: impl AsRef<Path>, params: CipherParams) -> Result<u64> {
    let source = source.as_ref();
    let size = std::fs::metadata(source)
        .map_err(|error| CoreError::Io {
            path: source.to_path_buf(),
            source: error,
        })?
        .len();
    Ok(size.div_ceil(params.page_size() as u64))
}

/// 读满缓冲区或读到文件末尾, 返回实际读到的字节数.
fn read_full(reader: &mut impl Read, buffer: &mut [u8], path: &Path) -> Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = reader
            .read(&mut buffer[filled..])
            .map_err(|error| CoreError::Io {
                path: path.to_path_buf(),
                source: error,
            })?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}
