//! Error type for the vendored resource / KV3 code (trimmed from morphic's
//! `error.rs`: texture, mesh and material variants removed).

#[derive(thiserror::Error, Debug)]
pub enum DecodeError {
    #[error("input too short at offset {offset}: needed {needed} bytes, had {had}")]
    Truncated {
        offset: u64,
        needed: usize,
        had: usize,
    },

    #[error("malformed resource header: {0}")]
    BadResource(&'static str),

    #[error("unsupported texture: {0}")]
    Texture(String),

    #[error("unsupported sound: {0}")]
    Sound(String),

    #[error("resource has no DATA block")]
    MissingDataBlock,

    #[error("unsupported KV3 version: 0x{0:08x}")]
    UnsupportedKv3(u32),

    #[error("KV3 parse error: {0}")]
    Kv3(&'static str),

    #[error("unsupported KV3 compression method: {0} (only 0=none, 1=LZ4, 2=ZSTD are handled)")]
    Kv3Compression(u32),

    #[error("KV3 LZ4 decompress failed: {0}")]
    Kv3Lz4(String),

    #[error("unknown KV3 node type: {0}")]
    Kv3NodeType(u8),

    #[error("model decode error: {0}")]
    Model(&'static str),
}
