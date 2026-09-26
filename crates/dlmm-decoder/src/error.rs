use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdlError {
    #[error("invalid IDL JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid IDL: {0}")]
    Invalid(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecodeError {
    #[error("data shorter than the 8-byte discriminator ({0} bytes)")]
    TooShort(usize),
    #[error("unknown discriminator {}", hex::encode(.0))]
    UnknownDiscriminator([u8; 8]),
    #[error("unexpected end of data while decoding {context} at byte {offset}")]
    Eof { context: String, offset: usize },
    #[error("invalid enum tag {tag} for {ty}")]
    InvalidEnumTag { ty: String, tag: u8 },
    #[error("invalid bool byte {0}")]
    InvalidBool(u8),
}
