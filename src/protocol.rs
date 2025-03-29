use crate::data_type::{datum_to_scalar, read_scalar_value, write_scalar_value, EncodedType};
use crate::error::FusionError;
use crate::ipc::{Bus, Slot, SlotNumber, SlotStream, DATA_SIZE};
use crate::worker::worker_id;
use anyhow::Result;
use datafusion::scalar::ScalarValue;
use datafusion_sql::TableReference;
use pgrx::pg_sys::{Oid, ParamExternData, ProcSendSignal};
use pgrx::prelude::*;
use pgrx::{pg_guard, PgRelation};
use rmp::decode::{read_array_len, read_bin_len, read_pfix, read_str_len, read_u16, read_u8};
use rmp::encode::{
    write_array_len, write_bin_len, write_bool, write_pfix, write_str, write_u16, write_u32,
    write_u8, RmpWrite,
};

#[repr(u8)]
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Direction {
    #[default]
    ToWorker = 0,
    ToBackend = 1,
}

impl TryFrom<u8> for Direction {
    type Error = FusionError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        assert!(value < 128);
        match value {
            0 => Ok(Direction::ToWorker),
            1 => Ok(Direction::ToBackend),
            _ => Err(FusionError::Deserialize(
                "direction".to_string(),
                value.into(),
            )),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Packet {
    #[default]
    Ack = 0,
    Bind = 1,
    Failure = 2,
    Metadata = 3,
    Parse = 4,
}

impl TryFrom<u8> for Packet {
    type Error = FusionError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        assert!(value < 128);
        match value {
            0 => Ok(Packet::Ack),
            1 => Ok(Packet::Bind),
            2 => Ok(Packet::Failure),
            3 => Ok(Packet::Metadata),
            4 => Ok(Packet::Parse),
            _ => Err(FusionError::Deserialize("packet".to_string(), value.into())),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) enum Flag {
    More = 0,
    #[default]
    Last = 1,
}

impl TryFrom<u8> for Flag {
    type Error = FusionError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        assert!(value < 128);
        match value {
            0 => Ok(Flag::More),
            1 => Ok(Flag::Last),
            _ => Err(FusionError::Deserialize("flag".to_string(), value.into())),
        }
    }
}

#[derive(Default, Debug, PartialEq)]
pub(crate) struct Header {
    pub(crate) direction: Direction,
    pub(crate) packet: Packet,
    pub(crate) flag: Flag,
    pub(crate) length: u16,
}

impl Header {
    const fn estimate_size() -> usize {
        // direction (1 byte) + packet(1 byte) + flag (1 byte) + length (3 bytes)
        1 + 1 + 1 + 3
    }

    const fn payload_max_size() -> usize {
        DATA_SIZE - Self::estimate_size()
    }
}

fn signal(slot_id: SlotNumber, direction: Direction) {
    match direction {
        Direction::ToWorker => {
            unsafe { ProcSendSignal(worker_id()) };
        }
        Direction::ToBackend => {
            let id = Bus::new().slot(slot_id).owner();
            unsafe { ProcSendSignal(id) };
        }
    }
}

// HEADER

pub(crate) fn consume_header(stream: &mut SlotStream) -> Result<Header> {
    assert_eq!(stream.position(), 0);
    let direction = Direction::try_from(read_pfix(stream)?)?;
    let packet = Packet::try_from(read_pfix(stream)?)?;
    let flag = Flag::try_from(read_pfix(stream)?)?;
    let length = read_u16(stream)?;
    Ok(Header {
        direction,
        packet,
        flag,
        length,
    })
}

pub(crate) fn write_header(stream: &mut SlotStream, header: &Header) -> Result<()> {
    write_pfix(stream, header.direction.to_owned() as u8)?;
    write_pfix(stream, header.packet.to_owned() as u8)?;
    write_pfix(stream, header.flag.to_owned() as u8)?;
    write_u16(stream, header.length.to_owned())?;
    Ok(())
}

// PARSE

/// Reads the query from the stream, but leaves the stream position at the beginning of the query.
/// It is required to return the reference to the query bytes without copying them. It is the
/// caller's responsibility to move the stream position to the end of the query.
///
/// Returns the query and its length.
pub(crate) fn read_query(stream: &mut SlotStream) -> Result<(&str, u32)> {
    let len = read_str_len(stream)?;
    let buf = stream.look_ahead(len as usize)?;
    let query = std::str::from_utf8(buf)?;
    Ok((query, len))
}

fn prepare_query(stream: &mut SlotStream, query: &str) -> Result<()> {
    stream.reset();
    // slot: header - bin marker - bin length - query bytes
    let length = 1 + 1 + query.len();
    if length > Header::payload_max_size() {
        return Err(FusionError::PayloadTooLarge(query.len()).into());
    }
    let header = Header {
        direction: Direction::ToWorker,
        packet: Packet::Parse,
        length: length as u16,
        flag: Flag::Last,
    };
    write_header(stream, &header)?;
    write_str(stream, query)?;
    Ok(())
}

pub(crate) fn send_query(slot_id: SlotNumber, mut stream: SlotStream, query: &str) -> Result<()> {
    prepare_query(&mut stream, query)?;
    // Unlock the slot after writing the query.
    let _guard = Slot::from(stream);
    signal(slot_id, Direction::ToWorker);
    Ok(())
}

// BIND

fn prepare_params(stream: &mut SlotStream, params: &[ParamExternData]) -> Result<()> {
    stream.reset();
    // We don't know the length of the parameters yet. So we write an invalid header
    // to replace it with the correct one later.
    write_header(stream, &Header::default())?;
    let pos_init = stream.position();
    write_array_len(stream, u32::try_from(params.len())?)?;
    for param in params {
        let value = datum_to_scalar(param.value, param.ptype, param.isnull)?;
        write_scalar_value(stream, &value)?;
    }
    let pos_final = stream.position();
    let length = u16::try_from(pos_final - pos_init)?;
    let header = Header {
        direction: Direction::ToWorker,
        packet: Packet::Bind,
        length,
        flag: Flag::Last,
    };
    stream.reset();
    write_header(stream, &header)?;
    stream.rewind(length as usize)?;
    Ok(())
}

pub(crate) fn read_params(stream: &mut SlotStream) -> Result<Vec<ScalarValue>> {
    let len = read_array_len(stream)?;
    let mut params = Vec::with_capacity(len as usize);
    for _ in 0..len {
        let value = read_scalar_value(stream)?;
        params.push(value);
    }
    Ok(params)
}

pub(crate) fn send_params(
    slot_id: SlotNumber,
    mut stream: SlotStream,
    params: &[ParamExternData],
) -> Result<()> {
    prepare_params(&mut stream, params)?;
    // Unlock the slot after writing the parameters.
    let _guard = Slot::from(stream);
    signal(slot_id, Direction::ToWorker);
    Ok(())
}

// FAILURE

pub(crate) fn read_error(stream: &mut SlotStream) -> Result<String> {
    let len = read_str_len(stream)?;
    let buf = stream.look_ahead(len as usize)?;
    let message = std::str::from_utf8(buf)?.to_string();
    Ok(message)
}

fn prepare_error(stream: &mut SlotStream, message: &str) -> Result<()> {
    stream.reset();
    let length = 1 + 1 + u32::try_from(message.len())?;
    let header = Header {
        direction: Direction::ToBackend,
        packet: Packet::Failure,
        length: length as u16,
        flag: Flag::Last,
    };
    write_header(stream, &header)?;
    write_str(stream, message)?;
    Ok(())
}

pub(crate) fn send_error(slot_id: SlotNumber, mut stream: SlotStream, message: &str) -> Result<()> {
    prepare_error(&mut stream, message)?;
    // Unlock the slot after writing the error message.
    let _guard = Slot::from(stream);
    signal(slot_id, Direction::ToBackend);
    Ok(())
}

#[inline]
fn write_c_str(stream: &mut SlotStream, s: &str) -> Result<()> {
    let len = u32::try_from(s.len())?;
    write_bin_len(stream, len + 1)?;
    stream.write_bytes(s.as_bytes())?;
    write_pfix(stream, 0)?;
    Ok(())
}

// METADATA

/// Writes a table reference as null-terminated strings to
/// the stream. It would be used by the Rust wrappers to the
/// C code, so if we serialize the table and schema as
/// null-terminated strings, we can avoid copying on
/// deserialization.
#[inline]
fn write_table_ref(stream: &mut SlotStream, table: &TableReference) -> Result<()> {
    match table {
        TableReference::Bare { table } => {
            write_array_len(stream, 1)?;
            write_c_str(stream, table)?;
        }
        TableReference::Full { schema, table, .. } | TableReference::Partial { schema, table } => {
            write_array_len(stream, 2)?;
            write_c_str(stream, schema)?;
            write_c_str(stream, table)?;
        }
    }
    Ok(())
}

pub(crate) fn prepare_table_refs(
    stream: &mut SlotStream,
    tables: &[&TableReference],
) -> Result<()> {
    stream.reset();
    // We don't know the length of the tables yet. So we write an invalid header
    // to replace it with the correct one later.
    write_header(stream, &Header::default())?;
    let pos_init = stream.position();
    write_array_len(stream, u32::try_from(tables.len())?)?;
    for table in tables {
        write_table_ref(stream, table)?;
    }
    let pos_final = stream.position();
    let length = u16::try_from(pos_final - pos_init)?;
    let header = Header {
        direction: Direction::ToBackend,
        packet: Packet::Metadata,
        length,
        flag: Flag::Last,
    };
    stream.reset();
    write_header(stream, &header)?;
    stream.rewind(length as usize)?;
    Ok(())
}

pub(crate) fn send_table_refs(
    slot_id: SlotNumber,
    mut stream: SlotStream,
    tables: &[&TableReference],
) -> Result<()> {
    prepare_table_refs(&mut stream, tables)?;
    // Unlock the slot after writing the table references.
    let _guard = Slot::from(stream);
    signal(slot_id, Direction::ToWorker);
    Ok(())
}

#[inline]
#[pg_guard]
fn serialize_table(rel_oid: Oid, stream: &mut SlotStream) -> Result<()> {
    // The destructor will release the lock.
    let rel = unsafe { PgRelation::with_lock(rel_oid, pg_sys::AccessShareLock as i32) };
    let tuple_desc = rel.tuple_desc();
    let attr_num = u32::try_from(tuple_desc.iter().filter(|a| !a.is_dropped()).count())?;
    write_u32(stream, rel_oid.as_u32())?;
    write_array_len(stream, attr_num)?;
    for attr in tuple_desc.iter() {
        if attr.is_dropped() {
            continue;
        }
        let etype = EncodedType::try_from(attr.type_oid().value())?;
        let is_nullable = !attr.attnotnull;
        let name = attr.name();
        write_array_len(stream, 3)?;
        write_str(stream, name)?;
        write_u8(stream, etype as u8)?;
        write_bool(stream, is_nullable)?;
    }
    Ok(())
}

pub(crate) fn prepare_metadata(rel_oids: &[Oid], stream: &mut SlotStream) -> Result<()> {
    stream.reset();
    // We don't know the length of the table metadata yet. So we write
    // an invalid header to replace it with the correct one later.
    write_header(stream, &Header::default())?;
    let pos_init = stream.position();
    write_array_len(stream, rel_oids.len() as u32)?;
    for &rel_oid in rel_oids {
        serialize_table(rel_oid, stream)?;
    }
    let pos_final = stream.position();
    let length = u16::try_from(pos_final - pos_init)?;
    let header = Header {
        direction: Direction::ToWorker,
        packet: Packet::Metadata,
        length,
        flag: Flag::Last,
    };
    stream.reset();
    write_header(stream, &header)?;
    stream.rewind(length as usize)?;
    Ok(())
}

pub(crate) fn send_metadata(
    slot_id: SlotNumber,
    mut stream: SlotStream,
    rel_oids: &[Oid],
) -> Result<()> {
    prepare_metadata(rel_oids, &mut stream)?;
    // Unlock the slot after writing the metadata.
    let _guard = Slot::from(stream);
    signal(slot_id, Direction::ToWorker);
    Ok(())
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use super::*;
    use pgrx::pg_sys::{Datum, Oid};
    use rmp::decode::{read_bool, read_u32};
    use std::ptr::addr_of_mut;

    const SLOT_SIZE: usize = 8204;

    #[pg_test]
    fn test_header() {
        let header = Header {
            direction: Direction::ToWorker,
            packet: Packet::Ack,
            length: 42,
            flag: Flag::Last,
        };
        let mut slot_buf: [u8; SLOT_SIZE] = [1; SLOT_SIZE];
        let ptr = addr_of_mut!(slot_buf) as *mut u8;
        Slot::init(ptr, slot_buf.len());
        let slot = Slot::from_bytes(ptr, slot_buf.len());
        let mut stream: SlotStream = slot.into();
        write_header(&mut stream, &header).unwrap();
        stream.reset();
        let new_header = consume_header(&mut stream).unwrap();
        assert_eq!(header, new_header);
    }

    #[pg_test]
    fn test_query() {
        let mut slot_buf: [u8; SLOT_SIZE] = [1; SLOT_SIZE];
        let ptr = addr_of_mut!(slot_buf) as *mut u8;
        Slot::init(ptr, slot_buf.len());
        let slot = Slot::from_bytes(ptr, slot_buf.len());
        let sql = "SELECT 1";
        let mut stream: SlotStream = slot.into();
        prepare_query(&mut stream, sql).unwrap();
        stream.reset();
        let header = consume_header(&mut stream).unwrap();
        assert_eq!(header.direction, Direction::ToWorker);
        assert_eq!(header.packet, Packet::Parse);
        assert_eq!(header.flag, Flag::Last);
        assert_eq!(header.length, 2 + sql.len() as u16);
        let (query, len) = read_query(&mut stream).unwrap();
        assert_eq!(query, sql);
        assert_eq!(len as usize, sql.len());
    }

    #[pg_test]
    fn test_params() {
        let mut slot_buf: [u8; SLOT_SIZE] = [1; SLOT_SIZE];
        let ptr = addr_of_mut!(slot_buf) as *mut u8;
        Slot::init(ptr, slot_buf.len());
        let slot = Slot::from_bytes(ptr, slot_buf.len());
        let mut stream: SlotStream = slot.into();
        let p1 = ParamExternData {
            value: Datum::from(1),
            ptype: pg_sys::INT4OID,
            isnull: false,
            pflags: 0,
        };
        let p2 = ParamExternData {
            value: Datum::from(0),
            ptype: pg_sys::INT4OID,
            isnull: true,
            pflags: 0,
        };
        prepare_params(&mut stream, &[p1, p2]).unwrap();
        stream.reset();
        let header = consume_header(&mut stream).unwrap();
        assert_eq!(header.direction, Direction::ToWorker);
        assert_eq!(header.packet, Packet::Bind);
        assert_eq!(header.flag, Flag::Last);
        let params = read_params(&mut stream).unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], ScalarValue::Int32(Some(1)));
        assert_eq!(params[1], ScalarValue::Int32(None));
    }

    #[pg_test]
    fn test_error() {
        let mut slot_buf: [u8; SLOT_SIZE] = [1; SLOT_SIZE];
        let ptr = addr_of_mut!(slot_buf) as *mut u8;
        Slot::init(ptr, slot_buf.len());
        let slot = Slot::from_bytes(ptr, slot_buf.len());
        let message = "An error occurred";
        let mut stream: SlotStream = slot.into();
        prepare_error(&mut stream, message).unwrap();
        stream.reset();
        let header = consume_header(&mut stream).unwrap();
        assert_eq!(header.direction, Direction::ToBackend);
        assert_eq!(header.packet, Packet::Failure);
        assert_eq!(header.flag, Flag::Last);
        assert_eq!(header.length, 2 + message.len() as u16);
        let error = read_error(&mut stream).unwrap();
        assert_eq!(error, message);
    }

    #[pg_test]
    fn test_table_request() {
        let mut slot_buf: [u8; SLOT_SIZE] = [1; SLOT_SIZE];
        let ptr = addr_of_mut!(slot_buf) as *mut u8;
        Slot::init(ptr, slot_buf.len());
        let slot = Slot::from_bytes(ptr, slot_buf.len());
        let mut stream: SlotStream = slot.into();
        let t1 = TableReference::bare("table1");
        let t2 = TableReference::partial("schema", "table2");
        let tables = vec![&t1, &t2];
        prepare_table_refs(&mut stream, &tables).unwrap();
        stream.reset();
        let header = consume_header(&mut stream).unwrap();
        assert_eq!(header.direction, Direction::ToBackend);
        assert_eq!(header.packet, Packet::Metadata);
        assert_eq!(header.flag, Flag::Last);

        // check table deserialization
        let table_num = read_array_len(&mut stream).unwrap();
        assert_eq!(table_num, 2);
        // table1
        let elem_num = read_array_len(&mut stream).unwrap();
        assert_eq!(elem_num, 1);
        let t1_len = read_bin_len(&mut stream).unwrap();
        assert_eq!(t1_len as usize, "table1".len() + 1);
        let t1 = stream.look_ahead(t1_len as usize).unwrap();
        assert_eq!(t1, b"table1\0");
        stream.rewind(t1_len as usize).unwrap();
        // schema.table2
        let elem_num = read_array_len(&mut stream).unwrap();
        assert_eq!(elem_num, 2);
        let s_len = read_bin_len(&mut stream).unwrap();
        assert_eq!(s_len as usize, "schema".len() + 1);
        let s = stream.look_ahead(s_len as usize).unwrap();
        assert_eq!(s, b"schema\0");
        stream.rewind(s_len as usize).unwrap();
        let t2_len = read_bin_len(&mut stream).unwrap();
        assert_eq!(t2_len as usize, "table2".len() + 1);
        let t2 = stream.look_ahead(t2_len as usize).unwrap();
        assert_eq!(t2, b"table2\0");
    }

    #[pg_test]
    fn test_metadata_response() {
        Spi::run("create table if not exists t1(a int not null, b text);").unwrap();
        let t1_oid = Spi::get_one::<Oid>("select 't1'::regclass::oid;")
            .unwrap()
            .unwrap();

        let mut slot_buf: [u8; SLOT_SIZE] = [1; SLOT_SIZE];
        let ptr = addr_of_mut!(slot_buf) as *mut u8;
        Slot::init(ptr, slot_buf.len());
        let slot = Slot::from_bytes(ptr, slot_buf.len());
        let mut stream: SlotStream = slot.into();

        prepare_metadata(&[t1_oid], &mut stream).unwrap();
        stream.reset();
        let header = consume_header(&mut stream).unwrap();
        assert_eq!(header.direction, Direction::ToWorker);
        assert_eq!(header.packet, Packet::Metadata);
        assert_eq!(header.flag, Flag::Last);

        // Check table metadata deserialization
        let table_num = read_array_len(&mut stream).unwrap();
        assert_eq!(table_num, 1);
        // t1
        let oid = read_u32(&mut stream).unwrap();
        assert_eq!(oid, t1_oid.as_u32());
        let attr_num = read_array_len(&mut stream).unwrap();
        assert_eq!(attr_num, 2);
        // a
        let elem_num = read_array_len(&mut stream).unwrap();
        assert_eq!(elem_num, 3);
        let name_len = read_str_len(&mut stream).unwrap();
        assert_eq!(name_len, "a".len() as u32);
        let name = stream.look_ahead(name_len as usize).unwrap();
        assert_eq!(name, b"a");
        stream.rewind(name_len as usize).unwrap();
        let etype = read_u8(&mut stream).unwrap();
        assert_eq!(etype, EncodedType::Int32 as u8);
        let is_nullable = read_bool(&mut stream).unwrap();
        assert!(!is_nullable);
        // b
        let elem_num = read_array_len(&mut stream).unwrap();
        assert_eq!(elem_num, 3);
        let name_len = read_str_len(&mut stream).unwrap();
        assert_eq!(name_len, "b".len() as u32);
        let name = stream.look_ahead(name_len as usize).unwrap();
        assert_eq!(name, b"b");
        stream.rewind(name_len as usize).unwrap();
        let etype = read_u8(&mut stream).unwrap();
        assert_eq!(etype, EncodedType::Utf8 as u8);
        let is_nullable = read_bool(&mut stream).unwrap();
        assert!(is_nullable);
    }
}
