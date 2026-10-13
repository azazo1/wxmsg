//! SQLite B 树页解析.
//!
//! 用于从**任意一个页版本**里直接解出行 —— 包括 WAL 里被取代的旧页。SQLite 的查询接口只能
//! 看到当前状态, 而撤回删掉的行只存在于旧页里, 所以必须自己按页解。
//!
//! 页头 (页 1 的页头在偏移 100, 其它页在偏移 0):
//!
//! ```text
//! 0      页类型 (2 内索引, 5 内表, 10 叶索引, 13 叶表)
//! 1..3   第一个空闲块
//! 3..5   单元数
//! 5..7   单元内容区起始
//! 7      碎片字节数
//! 8..12  最右子页 (仅内页)
//! ```
//!
//! 紧随页头是单元指针数组, 每项 2 字节大端, 值为单元相对页首的偏移。

pub mod record;

use crate::error::{CoreError, Result};

pub use record::{Value, decode_record, read_varint};

/// 内部索引页.
pub const PAGE_INTERIOR_INDEX: u8 = 2;
/// 内部表页.
pub const PAGE_INTERIOR_TABLE: u8 = 5;
/// 叶子索引页.
pub const PAGE_LEAF_INDEX: u8 = 10;
/// 叶子表页.
pub const PAGE_LEAF_TABLE: u8 = 13;

/// 页 1 的页头偏移, 前面是 100 字节的文件头.
const PAGE_ONE_HEADER_OFFSET: usize = 100;

/// 叶页里一条单元能全部放下时, 载荷大小的上限相对可用空间的余量.
const MAX_LEAF_SLACK: usize = 35;

/// 兜底上限, 防止损坏的页把遍历带进死循环.
const MAX_VISITED_PAGES: usize = 1 << 20;

/// 页头.
#[derive(Debug, Clone, Copy)]
pub struct PageHeader {
    /// 页类型.
    pub page_type: u8,
    /// 单元数.
    pub cells: usize,
    /// 页头在页内的偏移.
    pub header_offset: usize,
}

impl PageHeader {
    /// 解析页头.
    pub fn parse(page: &[u8], page_number: u64) -> Result<Self> {
        let header_offset = if page_number == 1 {
            PAGE_ONE_HEADER_OFFSET
        } else {
            0
        };
        let header =
            page.get(header_offset..header_offset + 8)
                .ok_or_else(|| CoreError::PageTooShort {
                    page: page_number,
                    needed: header_offset + 8,
                    actual: page.len(),
                })?;
        Ok(Self {
            page_type: header[0],
            cells: u16::from_be_bytes([header[3], header[4]]) as usize,
            header_offset,
        })
    }

    /// 是否叶子表页.
    pub fn is_leaf_table(&self) -> bool {
        self.page_type == PAGE_LEAF_TABLE
    }

    /// 是否内部表页.
    pub fn is_interior_table(&self) -> bool {
        self.page_type == PAGE_INTERIOR_TABLE
    }

    /// 单元指针数组的起始偏移.
    fn pointer_offset(&self) -> usize {
        self.header_offset
            + if self.page_type == PAGE_INTERIOR_TABLE {
                12
            } else {
                8
            }
    }
}

/// 叶页里的一条单元.
#[derive(Debug, Clone)]
pub struct LeafCell {
    /// 行号.
    pub rowid: i64,
    /// 载荷字节; 溢出时为页内局部部分, 不含溢出页内容.
    pub payload: Vec<u8>,
    /// 载荷的总长度 (含溢出部分).
    pub payload_size: usize,
    /// 载荷是否溢出到其它页.
    pub overflow: bool,
}

/// 取出叶页里的全部单元.
pub fn leaf_cells(page: &[u8], page_number: u64, usable_size: usize) -> Result<Vec<LeafCell>> {
    let header = PageHeader::parse(page, page_number)?;
    if !header.is_leaf_table() {
        return Err(CoreError::NotLeafPage {
            page: page_number,
            page_type: header.page_type,
        });
    }

    let mut cells = Vec::with_capacity(header.cells);
    for index in 0..header.cells {
        let pointer_at = header.pointer_offset() + index * 2;
        let pointer =
            page.get(pointer_at..pointer_at + 2)
                .ok_or_else(|| CoreError::PageTooShort {
                    page: page_number,
                    needed: pointer_at + 2,
                    actual: page.len(),
                })?;
        let offset = u16::from_be_bytes([pointer[0], pointer[1]]) as usize;

        let (payload_size, after_first) = read_varint(page, offset).ok_or_else(|| {
            CoreError::Record(format!(
                "页 {page_number} 第 {} 个单元的载荷长度 varint 不完整",
                index + 1
            ))
        })?;
        let (rowid, after_second) = read_varint(page, offset + after_first).ok_or_else(|| {
            CoreError::Record(format!(
                "页 {page_number} 第 {} 个单元的行号 varint 不完整",
                index + 1
            ))
        })?;

        let payload_size = payload_size as usize;
        let local = local_payload_size(payload_size, usable_size);
        let start = offset + after_first + after_second;
        let end = start + local;
        if end > page.len() {
            return Err(CoreError::PageTooShort {
                page: page_number,
                needed: end,
                actual: page.len(),
            });
        }

        // 溢出时, 局部载荷的最后 4 字节是第一个溢出页的页号。
        let overflow = payload_size > max_leaf_payload(usable_size);
        let payload_end = if overflow { end - 4 } else { end };
        cells.push(LeafCell {
            rowid: rowid as i64,
            payload: page[start..payload_end].to_vec(),
            payload_size,
            overflow,
        });
    }
    Ok(cells)
}

/// 内部表页里的全部子页号 (含最右子页).
pub fn child_pages(page: &[u8], page_number: u64) -> Result<Vec<u32>> {
    let header = PageHeader::parse(page, page_number)?;
    if !header.is_interior_table() {
        return Err(CoreError::NotInteriorPage {
            page: page_number,
            page_type: header.page_type,
        });
    }

    let mut children = Vec::with_capacity(header.cells + 1);
    for index in 0..header.cells {
        let pointer_at = header.pointer_offset() + index * 2;
        let pointer =
            page.get(pointer_at..pointer_at + 2)
                .ok_or_else(|| CoreError::PageTooShort {
                    page: page_number,
                    needed: pointer_at + 2,
                    actual: page.len(),
                })?;
        let offset = u16::from_be_bytes([pointer[0], pointer[1]]) as usize;
        let child = page
            .get(offset..offset + 4)
            .ok_or_else(|| CoreError::PageTooShort {
                page: page_number,
                needed: offset + 4,
                actual: page.len(),
            })?;
        children.push(u32::from_be_bytes([child[0], child[1], child[2], child[3]]));
    }

    // 页头 8..12 是最右子页。
    let right_most = page
        .get(header.header_offset + 8..header.header_offset + 12)
        .ok_or_else(|| CoreError::PageTooShort {
            page: page_number,
            needed: header.header_offset + 12,
            actual: page.len(),
        })?;
    children.push(u32::from_be_bytes([
        right_most[0],
        right_most[1],
        right_most[2],
        right_most[3],
    ]));
    Ok(children)
}

/// 叶页里能整段放下的载荷上限.
fn max_leaf_payload(usable_size: usize) -> usize {
    usable_size - MAX_LEAF_SLACK
}

/// 叶页单元的局部分段长度 (与 SQLite 的 maxLeaf / minLeaf 规则一致).
fn local_payload_size(payload_size: usize, usable_size: usize) -> usize {
    let max_local = max_leaf_payload(usable_size);
    if payload_size <= max_local {
        return payload_size;
    }
    let min_local = (usable_size - 12) * 32 / 255 - 23;
    let local = min_local + (payload_size - min_local) % (usable_size - 4);
    if local > max_local { min_local } else { local }
}

/// 按页号读取页内容的来源.
pub trait PageSource {
    /// 读一页.
    fn read_page(&self, page_number: u32) -> Result<Vec<u8>>;
}

/// 收集某张表 (以根页号指定) 的全部叶页.
pub fn leaf_pages(source: &impl PageSource, root: u32) -> Result<Vec<u32>> {
    let mut leaves = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut stack = vec![root];

    while let Some(page_number) = stack.pop() {
        if !visited.insert(page_number) {
            continue;
        }
        if visited.len() > MAX_VISITED_PAGES {
            return Err(CoreError::Record(
                "B 树遍历页数超出上限, 数据可能已损坏".into(),
            ));
        }
        let page = source.read_page(page_number)?;
        let header = PageHeader::parse(&page, page_number as u64)?;
        if header.is_leaf_table() {
            leaves.push(page_number);
        } else if header.is_interior_table() {
            stack.extend(child_pages(&page, page_number as u64)?);
        } else {
            // 页类型不是表页 (例如指向了空闲页) 说明这棵子树已经不完整。这里只跳过不中断:
            // 目标是尽量多地收集叶页, 而不是当校验器。
            tracing::warn!(
                page = page_number,
                page_type = header.page_type,
                "页类型不是表页, 跳过这棵子树"
            );
        }
    }

    leaves.sort_unstable();
    Ok(leaves)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USABLE: usize = 4016;

    /// 造一个只有一个单元的叶页.
    fn leaf_page_with(rowid: u64, payload: &[u8]) -> Vec<u8> {
        let mut page = vec![0u8; 4096];
        page[0] = PAGE_LEAF_TABLE;
        page[3..5].copy_from_slice(&1u16.to_be_bytes());

        // 单元放在页尾.
        let cell_offset = 4096 - (payload.len() + 3);
        page[8..10].copy_from_slice(&(cell_offset as u16).to_be_bytes());
        page[cell_offset] = payload.len() as u8;
        page[cell_offset + 1] = rowid as u8;
        page[cell_offset + 2..cell_offset + 2 + payload.len()].copy_from_slice(payload);
        page
    }

    #[test]
    fn decodes_leaf_cell() {
        let payload = [2u8, 1, 7]; // 一列, serial type 1, 值为 7
        let page = leaf_page_with(42, &payload);
        let cells = leaf_cells(&page, 2, USABLE).expect("应能解析");
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].rowid, 42);
        assert_eq!(cells[0].payload, payload);
        assert!(!cells[0].overflow);
        assert_eq!(
            decode_record(&cells[0].payload).unwrap(),
            vec![Value::Integer(7)]
        );
    }

    #[test]
    fn rejects_non_leaf_page() {
        let mut page = vec![0u8; 4096];
        page[0] = PAGE_INTERIOR_TABLE;
        assert!(matches!(
            leaf_cells(&page, 2, USABLE),
            Err(CoreError::NotLeafPage { .. })
        ));
    }

    #[test]
    fn computes_local_size_by_sqlite_rule() {
        // 完全不溢出.
        assert_eq!(local_payload_size(100, USABLE), 100);
        // 恰好等于上限.
        assert_eq!(local_payload_size(USABLE - 35, USABLE), USABLE - 35);
        // 溢出时按 minLeaf 取模; 结果超过上限则退回 minLeaf.
        let min_local = (USABLE - 12) * 32 / 255 - 23;
        let size = USABLE - 34;
        let modulo = min_local + (size - min_local) % (USABLE - 4);
        assert!(modulo > max_leaf_payload(USABLE), "本例应当触发上限回落");
        assert_eq!(local_payload_size(size, USABLE), min_local);
    }

    #[test]
    fn walks_leaf_pages() {
        // 根页是叶页本身.
        struct Single(Vec<u8>);
        impl PageSource for Single {
            fn read_page(&self, _page_number: u32) -> Result<Vec<u8>> {
                Ok(self.0.clone())
            }
        }
        let source = Single(leaf_page_with(1, &[2, 1, 1]));
        assert_eq!(leaf_pages(&source, 5).unwrap(), vec![5]);
    }
}
