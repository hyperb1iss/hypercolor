#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, LitInt, Result, parse_macro_input};

#[proc_macro_derive(BinaryFrame, attributes(frame, header, body, rest))]
pub fn derive_binary_frame(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    match expand_binary_frame(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand_binary_frame(input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let mut tag = None;
    let mut schema = None;

    for attr in &input.attrs {
        if !attr.path().is_ident("frame") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("tag") {
                let literal: LitInt = meta.value()?.parse()?;
                tag = Some(parse_u8_literal(&literal)?);
                return Ok(());
            }

            if meta.path.is_ident("schema") {
                let literal: LitInt = meta.value()?.parse()?;
                schema = Some(parse_u8_literal(&literal)?);
                return Ok(());
            }

            Err(meta.error("expected `tag = ...` or `schema = ...`"))
        })?;
    }

    let tag = tag.ok_or_else(|| {
        syn::Error::new_spanned(
            input,
            "missing `#[frame(tag = ..., schema = ...)]` attribute",
        )
    })?;
    let schema = schema.ok_or_else(|| {
        syn::Error::new_spanned(
            input,
            "missing `#[frame(tag = ..., schema = ...)]` attribute",
        )
    })?;

    Ok(quote! {
        impl ::hypercolor_leptos_ext::ws::BinaryFrameSchema for #name {
            const TAG: u8 = #tag;
            const SCHEMA: u8 = #schema;
            const NAME: &'static str = stringify!(#name);
        }
    })
}

fn parse_u8_literal(literal: &LitInt) -> Result<u8> {
    let raw = literal.to_string().replace('_', "");

    let parsed = if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u8::from_str_radix(hex, 16)
    } else if let Some(oct) = raw.strip_prefix("0o").or_else(|| raw.strip_prefix("0O")) {
        u8::from_str_radix(oct, 8)
    } else if let Some(bin) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
        u8::from_str_radix(bin, 2)
    } else {
        raw.parse::<u8>()
    };

    parsed.map_err(|_| syn::Error::new_spanned(literal, "value must fit in a `u8`"))
}
