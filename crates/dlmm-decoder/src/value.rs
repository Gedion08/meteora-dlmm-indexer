//! Borsh / bytemuck value decoding driven by the parsed IDL schema.
//!
//! Output conventions (stable — downstream SQL depends on them):
//! - integers up to 64 bits are JSON numbers; `u128`/`i128` are decimal strings
//!   (JSON consumers can't represent them losslessly as numbers);
//! - pubkeys are base58 strings, `bytes` are hex strings;
//! - unit enum variants are strings, data-carrying variants are `{ "Variant": ... }`;
//! - padding / reserved fields (names starting with `_` or `padding`) are skipped.
//!
//! Bytemuck (zero-copy) accounts are `Pod`, which forbids implicit padding, so their
//! on-chain layout is the plain sequential encoding of their fields — identical to
//! borsh for the fixed-size types they can contain.

use serde_json::{Map, Value};

use crate::error::DecodeError;
use crate::idl::{Fields, Schema, Ty, TypeDef};

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize, ctx: &dyn Fn() -> String) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::Eof {
                context: ctx(),
                offset: self.pos,
            });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn arr<const N: usize>(&mut self, ctx: &dyn Fn() -> String) -> Result<[u8; N], DecodeError> {
        Ok(self.take(N, ctx)?.try_into().expect("length checked"))
    }
}

pub(crate) fn decode_fields(
    schema: &Schema,
    fields: &Fields,
    r: &mut Reader,
    ctx: &str,
) -> Result<Value, DecodeError> {
    match fields {
        Fields::Unit => Ok(Value::Null),
        Fields::Named(fields) => {
            let mut map = Map::with_capacity(fields.len());
            for f in fields {
                let v = decode_ty(schema, &f.ty, r, &|| format!("{ctx}.{}", f.name))?;
                if !f.hidden {
                    map.insert(f.name.clone(), v);
                }
            }
            Ok(Value::Object(map))
        }
        Fields::Tuple(tys) => tys
            .iter()
            .enumerate()
            .map(|(i, t)| decode_ty(schema, t, r, &|| format!("{ctx}.{i}")))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
    }
}

pub(crate) fn decode_ty(
    schema: &Schema,
    ty: &Ty,
    r: &mut Reader,
    ctx: &dyn Fn() -> String,
) -> Result<Value, DecodeError> {
    Ok(match ty {
        Ty::Bool => match r.arr::<1>(ctx)?[0] {
            0 => Value::Bool(false),
            1 => Value::Bool(true),
            b => return Err(DecodeError::InvalidBool(b)),
        },
        Ty::U8 => r.arr::<1>(ctx)?[0].into(),
        Ty::I8 => (r.arr::<1>(ctx)?[0] as i8).into(),
        Ty::U16 => u16::from_le_bytes(r.arr(ctx)?).into(),
        Ty::I16 => i16::from_le_bytes(r.arr(ctx)?).into(),
        Ty::U32 => u32::from_le_bytes(r.arr(ctx)?).into(),
        Ty::I32 => i32::from_le_bytes(r.arr(ctx)?).into(),
        Ty::U64 => u64::from_le_bytes(r.arr(ctx)?).into(),
        Ty::I64 => i64::from_le_bytes(r.arr(ctx)?).into(),
        Ty::U128 => Value::String(u128::from_le_bytes(r.arr(ctx)?).to_string()),
        Ty::I128 => Value::String(i128::from_le_bytes(r.arr(ctx)?).to_string()),
        Ty::F32 => f32::from_le_bytes(r.arr(ctx)?).into(),
        Ty::F64 => f64::from_le_bytes(r.arr(ctx)?).into(),
        Ty::Pubkey => Value::String(bs58::encode(r.arr::<32>(ctx)?).into_string()),
        Ty::String => {
            let len = u32::from_le_bytes(r.arr(ctx)?) as usize;
            Value::String(String::from_utf8_lossy(r.take(len, ctx)?).into_owned())
        }
        Ty::Bytes => {
            let len = u32::from_le_bytes(r.arr(ctx)?) as usize;
            Value::String(hex::encode(r.take(len, ctx)?))
        }
        Ty::Array(elem, len) => {
            let mut out = Vec::with_capacity(*len);
            for _ in 0..*len {
                out.push(decode_ty(schema, elem, r, ctx)?);
            }
            Value::Array(out)
        }
        Ty::Vec(elem) => {
            let len = u32::from_le_bytes(r.arr(ctx)?) as usize;
            // Guard against garbage lengths allocating huge buffers.
            let mut out = Vec::with_capacity(len.min(r.remaining()));
            for _ in 0..len {
                out.push(decode_ty(schema, elem, r, ctx)?);
            }
            Value::Array(out)
        }
        Ty::Option(inner) => match r.arr::<1>(ctx)?[0] {
            0 => Value::Null,
            _ => decode_ty(schema, inner, r, ctx)?,
        },
        Ty::COption(inner) => {
            let tag = u32::from_le_bytes(r.arr(ctx)?);
            // COption always reserves space for the value.
            let v = decode_ty(schema, inner, r, ctx)?;
            if tag == 0 {
                Value::Null
            } else {
                v
            }
        }
        Ty::Defined(idx) => {
            let t = &schema.types[*idx];
            match &t.def {
                TypeDef::Struct(fields) => decode_fields(schema, fields, r, &t.name)?,
                TypeDef::Alias(inner) => decode_ty(schema, inner, r, ctx)?,
                TypeDef::Enum(variants) => {
                    let tag = r.arr::<1>(ctx)?[0];
                    let (name, fields) =
                        variants
                            .get(tag as usize)
                            .ok_or_else(|| DecodeError::InvalidEnumTag {
                                ty: t.name.clone(),
                                tag,
                            })?;
                    match fields {
                        Fields::Unit => Value::String(name.clone()),
                        _ => {
                            let mut m = Map::with_capacity(1);
                            m.insert(name.clone(), decode_fields(schema, fields, r, &t.name)?);
                            Value::Object(m)
                        }
                    }
                }
            }
        }
    })
}
