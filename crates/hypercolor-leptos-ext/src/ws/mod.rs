mod channel;
mod schema;
pub mod transport;

pub use channel::Channel;
pub use hypercolor_leptos_ext_macros::BinaryFrame;
pub use schema::{SchemaRange, negotiate_highest_common_schema};

pub trait BinaryFrameSchema {
    const TAG: u8;
    const SCHEMA: u8;
    const NAME: &'static str;
}

pub trait BinaryFrameMetadata: BinaryFrameSchema {}

impl<T: BinaryFrameSchema> BinaryFrameMetadata for T {}
