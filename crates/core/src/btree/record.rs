//! SQLite 记录格式: varint 与 serial type.
//!
//! 一条记录 (record) 是: 头长度 varint, 之后是各列的 serial type varint, 再之后是各列的值。
//! 值本身没有类型标记, 完全由 serial type 决定, 所以**必须按表的列顺序解释**。
//!
//! ```text
//! 记录 = [头长度 varint][serial type ...][值 ...]
//! ```

use crate::error::{CoreError, Result};

/// varint 最多 9 字节.
pub const MAX_VARINT_BYTES: usize = 9;

/// 记录里的一个值.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL NULL.
    Null,
    /// 整数.
    Integer(i64),
    /// 浮点数.
    Real(f64),
    /// 文本.
    Text(String),
    /// 二进制.
    Blob(Vec<u8>),
}

impl Value {
    /// 取整数值; 非整数返回 `None`.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// 取字节; 文本与二进制都返回原文.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Text(text) => Some(text.as_bytes()),
            Self::Blob(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// 转成便于显示的字符串.
    pub fn as_text(&self) -> Option<String> {
        match self {
            Self::Text(text) => Some(text.clone()),
            Self::Blob(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            Self::Integer(value) => Some(value.to_string()),
            Self::Real(value) => Some(value.to_string()),
            Self::Null => None,
        }
    }
}

/// 读一个 varint, 返回 (值, 占用字节数).
///
/// 前 8 字节各贡献 7 位; 第 9 字节贡献完整 8 位。
pub fn read_varint(data: &[u8], offset: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    for index in 0..MAX_VARINT_BYTES {
        let byte = *data.get(offset + index)?;
        if index == 8 {
            // 第 9 字节是完整的 8 位。
            return Some(((value << 8) | byte as u64, 9));
        }
        value = (value << 7) | (byte & 0x7f) as u64;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

/// 某个 serial type 对应的值长度.
fn serial_type_len(serial_type: u64) -> Option<usize> {
    match serial_type {
        0 => Some(0),
        1 => Some(1),
        2 => Some(2),
        3 => Some(3),
        4 => Some(4),
        5 => Some(6),
        6 => Some(8),
        7 => Some(8),
        8 | 9 => Some(0),
        10 | 11 => Some(0),
        value if value >= 12 => {
            let bytes = (value - 12) / 2;
            usize::try_from(bytes).ok()
        }
        _ => None,
    }
}

/// 按大端读有符号整数, 不足 8 字节时符号扩展.
fn read_signed(data: &[u8]) -> i64 {
    if data.is_empty() {
        return 0;
    }
    let negative = data[0] & 0x80 != 0;
    let mut value: i64 = if negative { -1 } else { 0 };
    for byte in data {
        value = (value << 8) | *byte as i64;
    }
    value
}

/// 解码一条记录.
pub fn decode_record(payload: &[u8]) -> Result<Vec<Value>> {
    let (header_size, header_varint) =
        read_varint(payload, 0).ok_or_else(|| CoreError::Record("头长度 varint 不完整".into()))?;
    let header_size = header_size as usize;
    if header_size < header_varint || header_size > payload.len() {
        return Err(CoreError::Record(format!(
            "记录头长度 {header_size} 超出载荷 {} 字节",
            payload.len()
        )));
    }

    let mut types = Vec::new();
    let mut offset = header_varint;
    while offset < header_size {
        let (serial_type, read) = read_varint(payload, offset)
            .ok_or_else(|| CoreError::Record("serial type varint 不完整".into()))?;
        offset += read;
        types.push(serial_type);
    }

    let mut values = Vec::with_capacity(types.len());
    let mut offset = header_size;
    for serial_type in types {
        let length = serial_type_len(serial_type)
            .ok_or_else(|| CoreError::Record(format!("未知的 serial type {serial_type}")))?;
        let end = offset + length;
        if end > payload.len() {
            return Err(CoreError::Record(format!(
                "第 {} 列需要 {length} 字节, 但载荷只剩 {} 字节",
                values.len() + 1,
                payload.len().saturating_sub(offset)
            )));
        }
        let bytes = &payload[offset..end];
        offset = end;

        let value = match serial_type {
            0 => Value::Null,
            1..=6 => Value::Integer(read_signed(bytes)),
            7 => Value::Real(f64::from_bits(u64::from_be_bytes(
                bytes.try_into().expect("8 字节"),
            ))),
            8 => Value::Integer(0),
            9 => Value::Integer(1),
            10 | 11 => Value::Null,
            value if value % 2 == 0 => Value::Blob(bytes.to_vec()),
            _ => Value::Text(String::from_utf8_lossy(bytes).into_owned()),
        };
        values.push(value);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_short_and_long_varints() {
        assert_eq!(read_varint(&[0x01], 0), Some((1, 1)));
        assert_eq!(read_varint(&[0x7f], 0), Some((127, 1)));
        assert_eq!(read_varint(&[0x81, 0x00], 0), Some((128, 2)));
        // 9 字节形式: 前 8 字节各给 7 位, 第 9 字节给满 8 位.
        // 八个 0x81 的 7 位段都是 1, 落在第 0,7,14,...,49 位, 即 0x0204_0810_2040_81.
        assert_eq!(
            read_varint(&[0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0xff], 0),
            Some((0x0204_0810_2040_81ff, 9))
        );
    }

    #[test]
    fn decodes_record_values() {
        // 头长度 4 (含长度字节本身), serial types: 1 (1 字节整数), 23 (5 字节文本), 8 (整数 0).
        let payload = [4u8, 1, 23, 8, 0x2a, b'h', b'e', b'l', b'l', b'o'];
        let values = decode_record(&payload).expect("应能解码");
        assert_eq!(values.len(), 3);
        assert_eq!(values[0], Value::Integer(42));
        assert_eq!(values[1], Value::Text("hello".into()));
        assert_eq!(values[2], Value::Integer(0));
    }

    #[test]
    fn sign_extends_negative_integers() {
        // serial type 1 配 0xff 应当读出 -1.
        let payload = [2u8, 1, 0xff];
        assert_eq!(
            decode_record(&payload).expect("应能解码"),
            vec![Value::Integer(-1)]
        );
    }

    #[test]
    fn rejects_inconsistent_header() {
        // 头长度声称 5, 但载荷不够.
        assert!(decode_record(&[5u8, 1]).is_err());
    }
}
