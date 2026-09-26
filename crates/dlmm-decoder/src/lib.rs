//! IDL-driven decoder for the Meteora DLMM (`lb_clmm`) program.
//!
//! Every instruction, `emit_cpi!` event and account type in the IDL is decoded into
//! JSON without hand-written layouts, so new IDL versions are a file swap rather than
//! a code change. See [`value`] for the JSON conventions.

mod error;
mod idl;
mod value;

use std::sync::OnceLock;

use serde_json::{Map, Value};

pub use error::{DecodeError, IdlError};
pub use idl::IxAccountDef;

use idl::{Fields, Schema};
use value::{decode_fields, Reader};

/// Meteora DLMM program ID on mainnet.
pub const PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";

/// Prefix of Anchor `emit_cpi!` self-invocations: `sha256("anchor:event")[..8]`.
pub const EVENT_IX_TAG: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];

const BUNDLED_IDL: &str = include_str!("../../../idl/dlmm.json");

#[derive(Debug, Clone)]
pub struct DecodedInstruction {
    pub name: &'static str,
    /// `{ idl_account_name: base58 | null }`. Optional accounts that were passed as the
    /// program ID (Anchor's "None" sentinel) are `null`.
    pub accounts: Value,
    /// Accounts beyond those declared in the IDL (e.g. bin arrays, transfer-hook accounts).
    pub remaining_accounts: Vec<String>,
    pub args: Value,
    pub trailing_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct DecodedEvent {
    pub name: &'static str,
    pub data: Value,
}

#[derive(Debug, Clone)]
pub struct DecodedAccount {
    pub name: &'static str,
    pub data: Value,
    /// Bytes after the fixed layout, e.g. extended bins on resized positions.
    pub trailing_bytes: usize,
}

pub struct Decoder {
    schema: Schema,
    program_id: [u8; 32],
    // Leaked once at startup so decoded records can carry `&'static str` names cheaply.
    ix_names: Vec<&'static str>,
    account_names: Vec<&'static str>,
    event_names: Vec<&'static str>,
}

fn leak_names<'a>(it: impl Iterator<Item = &'a String>) -> Vec<&'static str> {
    it.map(|s| &*Box::leak(s.clone().into_boxed_str())).collect()
}

fn split_disc(data: &[u8]) -> Result<([u8; 8], &[u8]), DecodeError> {
    if data.len() < 8 {
        return Err(DecodeError::TooShort(data.len()));
    }
    Ok((data[..8].try_into().expect("len checked"), &data[8..]))
}

impl Decoder {
    /// Build a decoder from an Anchor IDL JSON string.
    pub fn from_idl_json(json: &str) -> Result<Self, IdlError> {
        let schema = Schema::parse(json)?;
        let program_id: [u8; 32] = bs58::decode(&schema.address)
            .into_vec()
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| IdlError::Invalid(format!("bad program address {}", schema.address)))?;
        Ok(Self {
            ix_names: leak_names(schema.instructions.iter().map(|i| &i.name)),
            account_names: leak_names(schema.accounts.iter().map(|a| &a.name)),
            event_names: leak_names(schema.events.iter().map(|e| &e.name)),
            program_id,
            schema,
        })
    }

    /// Decoder for the IDL bundled at build time (`idl/dlmm.json`).
    pub fn bundled() -> &'static Decoder {
        static D: OnceLock<Decoder> = OnceLock::new();
        D.get_or_init(|| Decoder::from_idl_json(BUNDLED_IDL).expect("bundled IDL is valid"))
    }

    pub fn program_id(&self) -> &[u8; 32] {
        &self.program_id
    }

    pub fn program_id_str(&self) -> &str {
        &self.schema.address
    }

    pub fn idl_version(&self) -> &str {
        &self.schema.version
    }

    pub fn instruction_names(&self) -> &[&'static str] {
        &self.ix_names
    }

    pub fn account_names(&self) -> &[&'static str] {
        &self.account_names
    }

    pub fn event_names(&self) -> &[&'static str] {
        &self.event_names
    }

    /// 8-byte discriminator of an account type, for `getProgramAccounts` memcmp filters.
    pub fn account_discriminator(&self, name: &str) -> Option<[u8; 8]> {
        let idx = self.schema.accounts.iter().position(|a| a.name == name)?;
        self.schema
            .account_by_disc
            .iter()
            .find_map(|(d, i)| (*i == idx).then_some(*d))
    }

    /// True when instruction data is an Anchor `emit_cpi!` event rather than a real instruction.
    pub fn is_event_cpi(data: &[u8]) -> bool {
        data.len() >= 16 && data[..8] == EVENT_IX_TAG
    }

    /// Decode a DLMM instruction. `accounts` are the instruction's account pubkeys in order.
    pub fn decode_instruction(
        &self,
        data: &[u8],
        accounts: &[&[u8]],
    ) -> Result<DecodedInstruction, DecodeError> {
        let (disc, body) = split_disc(data)?;
        let idx = *self
            .schema
            .ix_by_disc
            .get(&disc)
            .ok_or(DecodeError::UnknownDiscriminator(disc))?;
        let ix = &self.schema.instructions[idx];

        let mut r = Reader::new(body);
        let mut args = Map::with_capacity(ix.args.len());
        for f in &ix.args {
            let v = value::decode_ty(&self.schema, &f.ty, &mut r, &|| format!("{}.{}", ix.name, f.name))?;
            if !f.hidden {
                args.insert(f.name.clone(), v);
            }
        }

        let mut named = Map::with_capacity(ix.accounts.len());
        for (def, key) in ix.accounts.iter().zip(accounts) {
            let v = if def.optional && *key == self.program_id {
                Value::Null
            } else {
                Value::String(bs58::encode(key).into_string())
            };
            named.insert(def.name.clone(), v);
        }
        let remaining_accounts = accounts
            .iter()
            .skip(ix.accounts.len())
            .map(|k| bs58::encode(k).into_string())
            .collect();

        Ok(DecodedInstruction {
            name: self.ix_names[idx],
            accounts: Value::Object(named),
            remaining_accounts,
            args: Value::Object(args),
            trailing_bytes: r.remaining(),
        })
    }

    /// Decode an `emit_cpi!` event from self-CPI instruction data (including the tag).
    pub fn decode_event_cpi(&self, data: &[u8]) -> Result<DecodedEvent, DecodeError> {
        let body = data.strip_prefix(&EVENT_IX_TAG).unwrap_or(data);
        self.decode_event(body)
    }

    /// Decode an event from `discriminator ++ borsh` bytes (e.g. a `Program data:` log).
    pub fn decode_event(&self, data: &[u8]) -> Result<DecodedEvent, DecodeError> {
        let (disc, body) = split_disc(data)?;
        let idx = *self
            .schema
            .event_by_disc
            .get(&disc)
            .ok_or(DecodeError::UnknownDiscriminator(disc))?;
        let ev = &self.schema.events[idx];
        let data = self.decode_type(ev.type_idx, body)?.0;
        Ok(DecodedEvent {
            name: self.event_names[idx],
            data,
        })
    }

    /// Decode account data owned by the DLMM program.
    pub fn decode_account(&self, data: &[u8]) -> Result<DecodedAccount, DecodeError> {
        let (disc, body) = split_disc(data)?;
        let idx = *self
            .schema
            .account_by_disc
            .get(&disc)
            .ok_or(DecodeError::UnknownDiscriminator(disc))?;
        let acc = &self.schema.accounts[idx];
        let (data, trailing_bytes) = self.decode_type(acc.type_idx, body)?;
        Ok(DecodedAccount {
            name: self.account_names[idx],
            data,
            trailing_bytes,
        })
    }

    /// Name of the account type for `data`, without decoding it.
    pub fn account_type(&self, data: &[u8]) -> Option<&'static str> {
        let (disc, _) = split_disc(data).ok()?;
        self.schema
            .account_by_disc
            .get(&disc)
            .map(|i| self.account_names[*i])
    }

    fn decode_type(&self, type_idx: usize, body: &[u8]) -> Result<(Value, usize), DecodeError> {
        let t = &self.schema.types[type_idx];
        let mut r = Reader::new(body);
        let fields = match &t.def {
            idl::TypeDef::Struct(f) => f,
            _ => &Fields::Unit,
        };
        let v = decode_fields(&self.schema, fields, &mut r, &t.name)?;
        Ok((v, r.remaining()))
    }
}
