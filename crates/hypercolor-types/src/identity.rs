//! Identity conventions (Spec 76 §4.2): the `uuid_id!` and `string_id!`
//! macros, the string-backed identifier set, and [`OutputRef`].
//!
//! Every identifier in the system is one of two shapes. UUID-backed ids
//! wrap a v7 UUID (time-ordered, safe as database and map keys).
//! String-backed ids are opaque strings whose grammar is validated at
//! construction — but never weakened to admit history: legacy persisted
//! forms enter through [`from_persisted`](LayoutId::from_persisted)
//! readers, not through loosened constructors.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::{PartialSchema, ToSchema};

use crate::device::DeviceId;

/// Why a string identifier failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} identifier: {reason}")]
pub struct IdParseError {
    /// The identifier type that rejected the input.
    pub kind: &'static str,
    /// What the input violated.
    pub reason: &'static str,
}

/// Generate a UUID-backed identifier newtype with the canonical impl
/// set: hyphenated `Display`, `Debug` as `Name(uuid)`, validating
/// `FromStr`, `AsRef<Uuid>`, and v7-minting `new`. Per-type extras
/// (`DEFAULT` consts, `Default` impls) stay in ordinary `impl` blocks
/// beside the invocation.
macro_rules! uuid_id {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
            ::utoipa::ToSchema,
        )]
        pub struct $name(pub ::uuid::Uuid);

        impl $name {
            /// Generate a fresh identifier (`UUIDv7` — time-ordered).
            #[must_use]
            pub fn new() -> Self {
                Self(::uuid::Uuid::now_v7())
            }

            /// Wrap an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: ::uuid::Uuid) -> Self {
                Self(uuid)
            }

            /// The inner UUID value.
            #[must_use]
            pub const fn as_uuid(&self) -> ::uuid::Uuid {
                self.0
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = ::uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                ::uuid::Uuid::parse_str(s).map(Self)
            }
        }

        impl ::core::convert::AsRef<::uuid::Uuid> for $name {
            fn as_ref(&self) -> &::uuid::Uuid {
                &self.0
            }
        }

        impl ::core::convert::From<::uuid::Uuid> for $name {
            fn from(uuid: ::uuid::Uuid) -> Self {
                Self(uuid)
            }
        }
    };
}

/// Generate a string-backed opaque identifier with a per-identity
/// validator. `new` and `FromStr` enforce the grammar for freshly
/// minted ids; `from_persisted` is the migration reader's door for
/// legacy forms and never validates — constructors are never weakened
/// to admit history.
macro_rules! string_id {
    ($(#[$attr:meta])* $name:ident, validate = $validate:path) => {
        $(#[$attr])*
        #[derive(
            Debug,
            Clone,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
            ::utoipa::ToSchema,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validate and wrap a freshly minted identifier.
            pub fn new(id: impl Into<String>) -> Result<Self, IdParseError> {
                let id = id.into();
                $validate(&id)?;
                Ok(Self(id))
            }

            /// Admit a legacy persisted identifier without validation.
            /// For migration readers ONLY — API inputs go through
            /// [`Self::new`] / `FromStr`.
            #[must_use]
            pub fn from_persisted(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// The identifier text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }

        impl ::core::convert::AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

pub(crate) use uuid_id;

fn validate_backend_id(s: &str) -> Result<(), IdParseError> {
    const KIND: &str = "backend";
    if s.is_empty() {
        return Err(IdParseError {
            kind: KIND,
            reason: "must not be empty",
        });
    }
    if s.len() > 64 {
        return Err(IdParseError {
            kind: KIND,
            reason: "must be 64 bytes or fewer",
        });
    }
    // The ':' exclusion is load-bearing: OutputRef's "backend:device"
    // wire form splits on the first ':'.
    if !s
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(IdParseError {
            kind: KIND,
            reason: "only lowercase ASCII, digits, '_' and '-' are allowed",
        });
    }
    Ok(())
}

fn validate_layout_id(s: &str) -> Result<(), IdParseError> {
    validate_opaque(s, "layout", 128)
}

fn validate_profile_id(s: &str) -> Result<(), IdParseError> {
    validate_opaque(s, "profile", 128)
}

fn validate_layout_device_id(s: &str) -> Result<(), IdParseError> {
    const KIND: &str = "layout device";
    if s.is_empty() {
        return Err(IdParseError {
            kind: KIND,
            reason: "must not be empty",
        });
    }
    if s.len() > 256 {
        return Err(IdParseError {
            kind: KIND,
            reason: "must be 256 bytes or fewer",
        });
    }
    if s.chars().any(char::is_control) {
        return Err(IdParseError {
            kind: KIND,
            reason: "control characters are not allowed",
        });
    }
    Ok(())
}

fn validate_opaque(s: &str, kind: &'static str, max: usize) -> Result<(), IdParseError> {
    if s.is_empty() {
        return Err(IdParseError {
            kind,
            reason: "must not be empty",
        });
    }
    if s.len() > max {
        return Err(IdParseError {
            kind,
            reason: "too long",
        });
    }
    if s.chars().any(char::is_control) {
        return Err(IdParseError {
            kind,
            reason: "control characters are not allowed",
        });
    }
    if s.trim() != s {
        return Err(IdParseError {
            kind,
            reason: "leading or trailing whitespace is not allowed",
        });
    }
    Ok(())
}

string_id!(
    /// Spatial layout identifier — `"default"` or a user slug.
    LayoutId,
    validate = validate_layout_id
);

string_id!(
    /// Saved profile identifier (`prof_…` as minted by the store).
    ProfileId,
    validate = validate_profile_id
);

string_id!(
    /// Driver-derived stable identifier for a device within a layout.
    /// Deliberately NOT rewritten to [`OutputRef`] — layout identity
    /// survives transport changes.
    LayoutDeviceId,
    validate = validate_layout_device_id
);

string_id!(
    /// Output backend identifier (`usb`, `wled`, …). Grammar forbids
    /// `':'` so the `backend:device` wire form is unambiguous.
    BackendId,
    validate = validate_backend_id
);

/// Physical output routing: which backend owns a device.
///
/// Wire form is `"backend:device"`, split on the FIRST `':'` only —
/// unambiguous because [`BackendId`]'s grammar forbids `':'`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutputRef {
    /// The owning backend.
    pub backend: BackendId,
    /// The device within that backend.
    pub device: DeviceId,
}

impl PartialSchema for OutputRef {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        // The wire form is the "backend:device" string, not the struct.
        String::schema()
    }
}

impl ToSchema for OutputRef {}

impl fmt::Display for OutputRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.backend, self.device)
    }
}

impl FromStr for OutputRef {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (backend, device) = s.split_once(':').ok_or(IdParseError {
            kind: "output ref",
            reason: "expected the form backend:device",
        })?;
        let backend = BackendId::from_str(backend)?;
        let device = DeviceId::from_str(device).map_err(|_| IdParseError {
            kind: "output ref",
            reason: "device segment is not a UUID",
        })?;
        Ok(OutputRef { backend, device })
    }
}

impl Serialize for OutputRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for OutputRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}
