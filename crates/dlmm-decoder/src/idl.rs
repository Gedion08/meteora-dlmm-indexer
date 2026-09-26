//! Parses an Anchor IDL (spec 0.1.0, Anchor >= 0.30) into a compact, index-based
//! schema the decoder can walk without string lookups on the hot path.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::error::IdlError;

#[derive(Debug, Clone)]
pub(crate) enum Ty {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    U128,
    I128,
    F32,
    F64,
    Pubkey,
    String,
    Bytes,
    Array(Box<Ty>, usize),
    Vec(Box<Ty>),
    Option(Box<Ty>),
    COption(Box<Ty>),
    Defined(usize),
}

#[derive(Debug, Clone)]
pub(crate) enum Fields {
    Named(Vec<Field>),
    Tuple(Vec<Ty>),
    Unit,
}

#[derive(Debug, Clone)]
pub(crate) struct Field {
    pub name: String,
    pub ty: Ty,
    /// Padding / reserved bytes are decoded (to advance the cursor) but not emitted.
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum TypeDef {
    Struct(Fields),
    Enum(Vec<(String, Fields)>),
    Alias(Ty),
}

#[derive(Debug, Clone)]
pub(crate) struct NamedType {
    pub name: String,
    pub def: TypeDef,
}

#[derive(Debug, Clone)]
pub struct IxAccountDef {
    pub name: String,
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct IxDef {
    pub name: String,
    pub accounts: Vec<IxAccountDef>,
    pub args: Vec<Field>,
}

#[derive(Debug, Clone)]
pub(crate) struct TypedDef {
    pub name: String,
    pub type_idx: usize,
}

#[derive(Debug)]
pub(crate) struct Schema {
    pub address: String,
    pub version: String,
    pub types: Vec<NamedType>,
    pub instructions: Vec<IxDef>,
    pub ix_by_disc: HashMap<[u8; 8], usize>,
    pub accounts: Vec<TypedDef>,
    pub account_by_disc: HashMap<[u8; 8], usize>,
    pub events: Vec<TypedDef>,
    pub event_by_disc: HashMap<[u8; 8], usize>,
}

#[derive(Deserialize)]
struct RawIdl {
    address: String,
    #[serde(default)]
    metadata: Option<RawMetadata>,
    instructions: Vec<RawIx>,
    #[serde(default)]
    accounts: Vec<RawDisc>,
    #[serde(default)]
    events: Vec<RawDisc>,
    #[serde(default)]
    types: Vec<RawTypeDef>,
}

#[derive(Deserialize)]
struct RawMetadata {
    #[serde(default)]
    version: String,
}

#[derive(Deserialize)]
struct RawIx {
    name: String,
    discriminator: Vec<u8>,
    accounts: Vec<RawIxAccount>,
    args: Vec<RawField>,
}

#[derive(Deserialize)]
struct RawIxAccount {
    name: String,
    #[serde(default)]
    optional: bool,
    /// Composite (nested) account groups; flattened in order.
    #[serde(default)]
    accounts: Option<Vec<RawIxAccount>>,
}

#[derive(Deserialize)]
struct RawDisc {
    name: String,
    discriminator: Vec<u8>,
}

#[derive(Deserialize)]
struct RawField {
    name: String,
    #[serde(rename = "type")]
    ty: Value,
}

#[derive(Deserialize)]
struct RawTypeDef {
    name: String,
    #[serde(rename = "type")]
    ty: RawTypeBody,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum RawTypeBody {
    Struct {
        #[serde(default)]
        fields: Option<Vec<Value>>,
    },
    Enum {
        variants: Vec<RawVariant>,
    },
    Type {
        alias: Value,
    },
}

#[derive(Deserialize)]
struct RawVariant {
    name: String,
    #[serde(default)]
    fields: Option<Vec<Value>>,
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('_') || name.starts_with("padding")
}

fn disc(name: &str, v: &[u8]) -> Result<[u8; 8], IdlError> {
    v.try_into()
        .map_err(|_| IdlError::Invalid(format!("{name}: discriminator must be 8 bytes")))
}

struct Resolver<'a> {
    names: &'a HashMap<String, usize>,
}

impl Resolver<'_> {
    fn ty(&self, v: &Value) -> Result<Ty, IdlError> {
        if let Some(s) = v.as_str() {
            return Ok(match s {
                "bool" => Ty::Bool,
                "u8" => Ty::U8,
                "i8" => Ty::I8,
                "u16" => Ty::U16,
                "i16" => Ty::I16,
                "u32" => Ty::U32,
                "i32" => Ty::I32,
                "u64" => Ty::U64,
                "i64" => Ty::I64,
                "u128" => Ty::U128,
                "i128" => Ty::I128,
                "f32" => Ty::F32,
                "f64" => Ty::F64,
                "pubkey" | "publicKey" => Ty::Pubkey,
                "string" => Ty::String,
                "bytes" => Ty::Bytes,
                other => return Err(IdlError::Invalid(format!("unsupported primitive `{other}`"))),
            });
        }
        let obj = v
            .as_object()
            .ok_or_else(|| IdlError::Invalid(format!("bad type: {v}")))?;
        if let Some(inner) = obj.get("vec") {
            return Ok(Ty::Vec(Box::new(self.ty(inner)?)));
        }
        if let Some(inner) = obj.get("option") {
            return Ok(Ty::Option(Box::new(self.ty(inner)?)));
        }
        if let Some(inner) = obj.get("coption") {
            return Ok(Ty::COption(Box::new(self.ty(inner)?)));
        }
        if let Some(arr) = obj.get("array").and_then(Value::as_array) {
            let len = arr
                .get(1)
                .and_then(Value::as_u64)
                .ok_or_else(|| IdlError::Invalid(format!("unsupported array length: {v}")))?;
            let elem = arr
                .first()
                .ok_or_else(|| IdlError::Invalid(format!("bad array: {v}")))?;
            return Ok(Ty::Array(Box::new(self.ty(elem)?), len as usize));
        }
        if let Some(d) = obj.get("defined") {
            let name = d
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| d.as_str())
                .ok_or_else(|| IdlError::Invalid(format!("bad defined type: {v}")))?;
            if d.get("generics").is_some_and(|g| g.as_array().is_some_and(|a| !a.is_empty())) {
                return Err(IdlError::Invalid(format!("generic type `{name}` not supported")));
            }
            let idx = *self
                .names
                .get(name)
                .ok_or_else(|| IdlError::Invalid(format!("unknown type `{name}`")))?;
            return Ok(Ty::Defined(idx));
        }
        Err(IdlError::Invalid(format!("unsupported type: {v}")))
    }

    fn fields(&self, raw: Option<&Vec<Value>>) -> Result<Fields, IdlError> {
        let Some(raw) = raw.filter(|f| !f.is_empty()) else {
            return Ok(Fields::Unit);
        };
        // Named fields are objects with a `name`; tuple fields are bare types.
        if raw[0].get("name").is_some() {
            raw.iter()
                .map(|f| {
                    let name = f["name"].as_str().unwrap_or_default().to_owned();
                    Ok(Field {
                        hidden: is_hidden(&name),
                        ty: self.ty(&f["type"])?,
                        name,
                    })
                })
                .collect::<Result<_, _>>()
                .map(Fields::Named)
        } else {
            raw.iter()
                .map(|t| self.ty(t))
                .collect::<Result<_, _>>()
                .map(Fields::Tuple)
        }
    }
}

fn flatten_accounts(raw: Vec<RawIxAccount>, out: &mut Vec<IxAccountDef>) {
    for a in raw {
        match a.accounts {
            Some(nested) => flatten_accounts(nested, out),
            None => out.push(IxAccountDef {
                name: a.name,
                optional: a.optional,
            }),
        }
    }
}

impl Schema {
    pub fn parse(json: &str) -> Result<Self, IdlError> {
        let raw: RawIdl = serde_json::from_str(json)?;

        let names: HashMap<String, usize> = raw
            .types
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.clone(), i))
            .collect();
        let r = Resolver { names: &names };

        let types = raw
            .types
            .iter()
            .map(|t| {
                let def = match &t.ty {
                    RawTypeBody::Struct { fields } => TypeDef::Struct(r.fields(fields.as_ref())?),
                    RawTypeBody::Enum { variants } => TypeDef::Enum(
                        variants
                            .iter()
                            .map(|v| Ok((v.name.clone(), r.fields(v.fields.as_ref())?)))
                            .collect::<Result<_, IdlError>>()?,
                    ),
                    RawTypeBody::Type { alias } => TypeDef::Alias(r.ty(alias)?),
                };
                Ok(NamedType {
                    name: t.name.clone(),
                    def,
                })
            })
            .collect::<Result<Vec<_>, IdlError>>()?;

        let mut instructions = Vec::with_capacity(raw.instructions.len());
        let mut ix_by_disc = HashMap::new();
        for (i, ix) in raw.instructions.into_iter().enumerate() {
            ix_by_disc.insert(disc(&ix.name, &ix.discriminator)?, i);
            let mut accounts = Vec::new();
            flatten_accounts(ix.accounts, &mut accounts);
            let args = ix
                .args
                .iter()
                .map(|a| {
                    Ok(Field {
                        hidden: is_hidden(&a.name),
                        name: a.name.clone(),
                        ty: r.ty(&a.ty)?,
                    })
                })
                .collect::<Result<_, IdlError>>()?;
            instructions.push(IxDef {
                name: ix.name,
                accounts,
                args,
            });
        }

        let typed = |list: Vec<RawDisc>| -> Result<(Vec<TypedDef>, HashMap<[u8; 8], usize>), IdlError> {
            let mut defs = Vec::with_capacity(list.len());
            let mut by_disc = HashMap::new();
            for (i, d) in list.into_iter().enumerate() {
                by_disc.insert(disc(&d.name, &d.discriminator)?, i);
                let type_idx = *names
                    .get(&d.name)
                    .ok_or_else(|| IdlError::Invalid(format!("no type definition for `{}`", d.name)))?;
                defs.push(TypedDef {
                    name: d.name,
                    type_idx,
                });
            }
            Ok((defs, by_disc))
        };
        let (accounts, account_by_disc) = typed(raw.accounts)?;
        let (events, event_by_disc) = typed(raw.events)?;

        Ok(Self {
            address: raw.address,
            version: raw.metadata.map(|m| m.version).unwrap_or_default(),
            types,
            instructions,
            ix_by_disc,
            accounts,
            account_by_disc,
            events,
            event_by_disc,
        })
    }
}
