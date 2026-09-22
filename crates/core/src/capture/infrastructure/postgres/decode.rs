//! Decoder for `pgoutput` logical-replication messages (protocol version 1, text mode).
//!
//! `pgwire-replication` deliberately stops at transaction boundaries (`Begin`/`Commit`) and
//! forwards everything else — `Relation`/`Insert`/`Update`/`Delete`/`Truncate` — as raw
//! `XLogData` bytes; decoding those is left to the consumer. This module is that decoder.
//!
//! Wire format reference: PostgreSQL docs, "Logical Streaming Replication Protocol",
//! pgoutput message formats. All multi-byte integers are big-endian.

use std::collections::HashMap;

use crate::capture::domain::ColumnValue;

/// Column names for a relation, cached from the last `Relation` message seen for that OID —
/// `Insert`/`Update`/`Delete` messages only carry the relation OID and tuple data, not the
/// column names, so this cache is required to make sense of them.
#[derive(Debug, Clone)]
pub struct RelationInfo {
    pub namespace: String,
    pub name: String,
    pub columns: Vec<String>,
}

/// A decoded row-changing message, still relation-OID-addressed — the caller resolves the
/// OID against its own `RelationInfo` cache (kept outside this module since it spans
/// multiple `XLogData` messages) to attach schema/table names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedChange {
    Insert {
        relation_oid: u32,
        after: Vec<ColumnValue>,
    },
    Update {
        relation_oid: u32,
        before: Option<Vec<ColumnValue>>,
        after: Vec<ColumnValue>,
    },
    Delete {
        relation_oid: u32,
        before: Vec<ColumnValue>,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("pgoutput message truncated: expected {expected} more byte(s), found {found}")]
    Truncated { expected: usize, found: usize },
    #[error("pgoutput message missing null terminator for a string field")]
    MissingNullTerminator,
    #[error("pgoutput message referenced unknown relation OID {0} (no prior Relation message)")]
    UnknownRelation(u32),
    #[error("pgoutput message had unexpected tuple marker byte {0:#04x}")]
    UnexpectedTupleMarker(u8),
    #[error("pgoutput message had unexpected column kind byte {0:#04x}")]
    UnexpectedColumnKind(u8),
    #[error("pgoutput message column value was not valid UTF-8")]
    InvalidUtf8,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let rest = self.remaining();
        if rest.len() < n {
            return Err(DecodeError::Truncated {
                expected: n,
                found: rest.len(),
            });
        }
        self.pos += n;
        Ok(&rest[..n])
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// Reads a null-terminated string (used for identifiers, which pgoutput always sends
    /// as C-strings rather than length-prefixed text).
    fn cstr(&mut self) -> Result<String, DecodeError> {
        let rest = self.remaining();
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(DecodeError::MissingNullTerminator)?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.pos += end + 1;
        Ok(s)
    }
}

/// Decodes a `Relation` message body (tag byte already consumed by the caller) and returns
/// its OID plus the [`RelationInfo`] to cache for later `Insert`/`Update`/`Delete` messages.
pub fn decode_relation(payload: &[u8]) -> Result<(u32, RelationInfo), DecodeError> {
    let mut r = Reader::new(payload);
    let oid = r.u32()?;
    let namespace = r.cstr()?;
    let name = r.cstr()?;
    let _replica_identity = r.u8()?;
    let column_count = r.u16()?;

    let mut columns = Vec::with_capacity(column_count as usize);
    for _ in 0..column_count {
        let _flags = r.u8()?;
        let column_name = r.cstr()?;
        let _type_oid = r.i32()?;
        let _type_modifier = r.i32()?;
        columns.push(column_name);
    }

    Ok((
        oid,
        RelationInfo {
            namespace,
            name,
            columns,
        },
    ))
}

/// Decodes an `Insert` message body (tag byte already consumed).
pub fn decode_insert(
    payload: &[u8],
    relations: &HashMap<u32, RelationInfo>,
) -> Result<DecodedChange, DecodeError> {
    let mut r = Reader::new(payload);
    let relation_oid = r.u32()?;
    let relation = relations
        .get(&relation_oid)
        .ok_or(DecodeError::UnknownRelation(relation_oid))?;

    expect_marker(&mut r, b'N')?;
    let after = decode_tuple_data(&mut r, &relation.columns)?;

    Ok(DecodedChange::Insert {
        relation_oid,
        after,
    })
}

/// Decodes an `Update` message body (tag byte already consumed).
pub fn decode_update(
    payload: &[u8],
    relations: &HashMap<u32, RelationInfo>,
) -> Result<DecodedChange, DecodeError> {
    let mut r = Reader::new(payload);
    let relation_oid = r.u32()?;
    let relation = relations
        .get(&relation_oid)
        .ok_or(DecodeError::UnknownRelation(relation_oid))?;

    let mut marker = r.u8()?;
    let before = if marker == b'K' || marker == b'O' {
        let before = decode_tuple_data(&mut r, &relation.columns)?;
        marker = r.u8()?;
        Some(before)
    } else {
        None
    };

    if marker != b'N' {
        return Err(DecodeError::UnexpectedTupleMarker(marker));
    }
    let after = decode_tuple_data(&mut r, &relation.columns)?;

    Ok(DecodedChange::Update {
        relation_oid,
        before,
        after,
    })
}

/// Decodes a `Delete` message body (tag byte already consumed).
pub fn decode_delete(
    payload: &[u8],
    relations: &HashMap<u32, RelationInfo>,
) -> Result<DecodedChange, DecodeError> {
    let mut r = Reader::new(payload);
    let relation_oid = r.u32()?;
    let relation = relations
        .get(&relation_oid)
        .ok_or(DecodeError::UnknownRelation(relation_oid))?;

    let marker = r.u8()?;
    if marker != b'K' && marker != b'O' {
        return Err(DecodeError::UnexpectedTupleMarker(marker));
    }
    let before = decode_tuple_data(&mut r, &relation.columns)?;

    Ok(DecodedChange::Delete {
        relation_oid,
        before,
    })
}

fn expect_marker(r: &mut Reader<'_>, expected: u8) -> Result<(), DecodeError> {
    let marker = r.u8()?;
    if marker != expected {
        return Err(DecodeError::UnexpectedTupleMarker(marker));
    }
    Ok(())
}

fn decode_tuple_data(
    r: &mut Reader<'_>,
    columns: &[String],
) -> Result<Vec<ColumnValue>, DecodeError> {
    let column_count = r.u16()? as usize;
    let mut values = Vec::with_capacity(column_count);

    for i in 0..column_count {
        let name = columns.get(i).cloned().unwrap_or_else(|| format!("${i}"));
        let kind = r.u8()?;
        let value = match kind {
            b'n' => None,
            b'u' => None,
            b't' | b'b' => {
                let len = r.u32()? as usize;
                let bytes = r.take(len)?;
                Some(
                    std::str::from_utf8(bytes)
                        .map_err(|_| DecodeError::InvalidUtf8)?
                        .to_string(),
                )
            }
            other => return Err(DecodeError::UnexpectedColumnKind(other)),
        };
        values.push(ColumnValue { name, value });
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cstr_bytes(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    }

    fn sample_relation(columns: &[&str]) -> RelationInfo {
        RelationInfo {
            namespace: "public".to_string(),
            name: "orders".to_string(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn build_relation_message(oid: u32, namespace: &str, name: &str, columns: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&oid.to_be_bytes());
        buf.extend_from_slice(&cstr_bytes(namespace));
        buf.extend_from_slice(&cstr_bytes(name));
        buf.push(b'd'); // replica identity: default
        buf.extend_from_slice(&(columns.len() as u16).to_be_bytes());
        for col in columns {
            buf.push(0); // flags: not a key column
            buf.extend_from_slice(&cstr_bytes(col));
            buf.extend_from_slice(&23i32.to_be_bytes()); // type oid: int4
            buf.extend_from_slice(&(-1i32).to_be_bytes()); // type modifier: none
        }
        buf
    }

    fn build_tuple_data(values: &[Option<&str>]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(values.len() as u16).to_be_bytes());
        for value in values {
            match value {
                None => buf.push(b'n'),
                Some(text) => {
                    buf.push(b't');
                    buf.extend_from_slice(&(text.len() as u32).to_be_bytes());
                    buf.extend_from_slice(text.as_bytes());
                }
            }
        }
        buf
    }

    #[test]
    fn decodes_relation_message() {
        let payload = build_relation_message(16412, "public", "orders", &["id", "total"]);
        let (oid, relation) = decode_relation(&payload).unwrap();

        assert_eq!(oid, 16412);
        assert_eq!(relation.namespace, "public");
        assert_eq!(relation.name, "orders");
        assert_eq!(relation.columns, vec!["id".to_string(), "total".to_string()]);
    }

    #[test]
    fn decodes_insert_message() {
        let mut relations = HashMap::new();
        relations.insert(1, sample_relation(&["id", "total"]));

        let mut payload = 1u32.to_be_bytes().to_vec();
        payload.push(b'N');
        payload.extend_from_slice(&build_tuple_data(&[Some("42"), Some("9.99")]));

        let change = decode_insert(&payload, &relations).unwrap();
        assert_eq!(
            change,
            DecodedChange::Insert {
                relation_oid: 1,
                after: vec![
                    ColumnValue {
                        name: "id".to_string(),
                        value: Some("42".to_string()),
                    },
                    ColumnValue {
                        name: "total".to_string(),
                        value: Some("9.99".to_string()),
                    },
                ],
            }
        );
    }

    #[test]
    fn decodes_update_message_without_old_tuple() {
        let mut relations = HashMap::new();
        relations.insert(1, sample_relation(&["id", "total"]));

        let mut payload = 1u32.to_be_bytes().to_vec();
        payload.push(b'N');
        payload.extend_from_slice(&build_tuple_data(&[Some("42"), Some("19.99")]));

        let change = decode_update(&payload, &relations).unwrap();
        assert_eq!(
            change,
            DecodedChange::Update {
                relation_oid: 1,
                before: None,
                after: vec![
                    ColumnValue {
                        name: "id".to_string(),
                        value: Some("42".to_string()),
                    },
                    ColumnValue {
                        name: "total".to_string(),
                        value: Some("19.99".to_string()),
                    },
                ],
            }
        );
    }

    #[test]
    fn decodes_update_message_with_old_tuple() {
        let mut relations = HashMap::new();
        relations.insert(1, sample_relation(&["id", "total"]));

        let mut payload = 1u32.to_be_bytes().to_vec();
        payload.push(b'O');
        payload.extend_from_slice(&build_tuple_data(&[Some("42"), Some("9.99")]));
        payload.push(b'N');
        payload.extend_from_slice(&build_tuple_data(&[Some("42"), Some("19.99")]));

        let change = decode_update(&payload, &relations).unwrap();
        let (before, after) = match change {
            DecodedChange::Update { before, after, .. } => (before, after),
            other => panic!("expected Update, got {other:?}"),
        };

        assert_eq!(before.unwrap()[1].value.as_deref(), Some("9.99"));
        assert_eq!(after[1].value.as_deref(), Some("19.99"));
    }

    #[test]
    fn decodes_delete_message() {
        let mut relations = HashMap::new();
        relations.insert(1, sample_relation(&["id", "total"]));

        let mut payload = 1u32.to_be_bytes().to_vec();
        payload.push(b'K');
        payload.extend_from_slice(&build_tuple_data(&[Some("42"), None]));

        let change = decode_delete(&payload, &relations).unwrap();
        assert_eq!(
            change,
            DecodedChange::Delete {
                relation_oid: 1,
                before: vec![
                    ColumnValue {
                        name: "id".to_string(),
                        value: Some("42".to_string()),
                    },
                    ColumnValue {
                        name: "total".to_string(),
                        value: None,
                    },
                ],
            }
        );
    }

    #[test]
    fn unknown_relation_is_rejected() {
        let relations = HashMap::new();
        let mut payload = 999u32.to_be_bytes().to_vec();
        payload.push(b'N');
        payload.extend_from_slice(&build_tuple_data(&[]));

        let err = decode_insert(&payload, &relations).unwrap_err();
        assert_eq!(err, DecodeError::UnknownRelation(999));
    }

    #[test]
    fn truncated_message_is_rejected() {
        let payload = vec![0u8, 0, 0]; // short of the 4-byte OID
        let err = decode_relation(&payload).unwrap_err();
        assert!(matches!(err, DecodeError::Truncated { .. }));
    }
}
