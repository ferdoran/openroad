use std::io::Read;

use byteorder::ReadBytesExt;
use bytes::{BufMut, BytesMut};

pub use error::SerializationError;

pub mod error;

macro_rules! implement_primitive {
    ($tt:ty, $read:ident) => {
        impl Serialize for $tt {
            fn serialize_to(&self, buf: &mut BytesMut) {
                buf.put_slice(&self.to_le_bytes());
            }
        }

        impl ByteSize for $tt {
            fn byte_size(&self) -> usize {
                std::mem::size_of::<$tt>()
            }
        }

        impl Deserialize for $tt {
            fn read_from<T: Read + ReadBytesExt>(
                reader: &mut T,
            ) -> Result<Self, SerializationError> {
                Ok(reader.$read::<byteorder::LittleEndian>()?)
            }
        }
    };
}

pub trait Serialize: ByteSize {
    fn serialize_to(&self, buf: &mut BytesMut);
}

pub trait Deserialize {
    fn read_from<T: Read + ReadBytesExt>(reader: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized;
}

pub trait ByteSize {
    fn byte_size(&self) -> usize;
}

impl Serialize for u8 {
    fn serialize_to(&self, buf: &mut BytesMut) {
        buf.put_u8(*self);
    }
}

impl ByteSize for u8 {
    fn byte_size(&self) -> usize {
        std::mem::size_of::<u8>()
    }
}

impl Deserialize for u8 {
    fn read_from<T: Read + ReadBytesExt>(reader: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(reader.read_u8()?)
    }
}

impl Serialize for bool {
    fn serialize_to(&self, buf: &mut BytesMut) {
        let value = if *self { 1 } else { 0u8 };
        value.serialize_to(buf);
    }
}

impl ByteSize for bool {
    fn byte_size(&self) -> usize {
        1
    }
}

impl Deserialize for bool {
    fn read_from<T: Read + ReadBytesExt>(reader: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(reader.read_u8()? == 1)
    }
}

/// An IPv4 address is four raw octets in wire order (`7f 00 00 01` =
/// 127.0.0.1), not a `u32`: the original prints them low-byte-first
/// (`0086bfc0:161-162`), so modelling it as an integer only re-opens the
/// byte-order question at every consumer.
impl Serialize for std::net::Ipv4Addr {
    fn serialize_to(&self, buf: &mut BytesMut) {
        buf.put_slice(&self.octets());
    }
}

impl ByteSize for std::net::Ipv4Addr {
    fn byte_size(&self) -> usize {
        4
    }
}

impl Deserialize for std::net::Ipv4Addr {
    fn read_from<T: Read + ReadBytesExt>(reader: &mut T) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let mut octets = [0u8; 4];
        reader.read_exact(&mut octets)?;
        Ok(std::net::Ipv4Addr::from(octets))
    }
}

implement_primitive!(u16, read_u16);
implement_primitive!(i16, read_i16);
implement_primitive!(u32, read_u32);
implement_primitive!(i32, read_i32);
implement_primitive!(u64, read_u64);
implement_primitive!(i64, read_i64);
implement_primitive!(f32, read_f32);
implement_primitive!(f64, read_f64);
