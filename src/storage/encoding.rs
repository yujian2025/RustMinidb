//! 主键序列化/反序列化
//!
//! 将 Value 序列化为字节时使用类型标记 + 大端序，
//! 保证 redb 的 BTree 排序正确性。

use crate::sql::types::Value;

/// 将 Value 序列化为字节，保证 redb 的 BTree 排序正确
/// 整数用大端序（big-endian），确保数字排序和字节序一致
pub fn serialize_value(val: &Value) -> Vec<u8> {
    match val {
        Value::Integer(v) => {
            let mut buf = vec![0x01]; // 类型标记
            buf.extend_from_slice(&v.to_be_bytes()); // 大端序
            buf
        }
        Value::Text(v) => {
            let mut buf = vec![0x02];
            buf.extend_from_slice(v.as_bytes());
            buf
        }
        Value::Null => vec![0x00],
        _ => {
            // Blob, Float, Boolean, Timestamp 作为主键暂不支持
            // 降级为 Text 序列化
            let mut buf = vec![0x02];
            buf.extend_from_slice(format!("{:?}", val).as_bytes());
            buf
        }
    }
}

/// 反序列化主键
pub fn deserialize_value(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return Value::Null;
    }
    match bytes[0] {
        0x00 => Value::Null,
        0x01 => {
            if bytes.len() < 9 {
                return Value::Null;
            }
            let arr: [u8; 8] = bytes[1..9].try_into().unwrap_or([0u8; 8]);
            Value::Integer(i64::from_be_bytes(arr))
        }
        0x02 => Value::Text(String::from_utf8_lossy(&bytes[1..]).to_string()),
        _ => Value::Null,
    }
}
