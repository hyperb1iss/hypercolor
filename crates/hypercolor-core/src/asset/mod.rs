//! User media asset library.

pub mod index;
pub mod library;
pub mod watcher;

pub use index::{
    AssetEvent, AssetIndex, AssetScanStatus, AssetWarning, INDEX_VERSION, MediaAssetRecord,
};
pub use library::{
    AssetLibrary, AssetLibraryError, AssetLibraryLimits, AssetMetadataUpdate, AssetTypeHint,
    AssetUploadOptions, AssetUpsert, StreamUrlPolicy, stream_url_from_bytes_with_policy,
};
pub use watcher::{AssetWatchEvent, AssetWatcher};
