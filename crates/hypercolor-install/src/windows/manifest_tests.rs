use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{MAX_VENDOR_SIGNED, PermissionClass, ValidatedManifest, WindowsVendor};

/// A change made to a manifest that must make it invalid.
type ManifestEdit = (&'static str, Box<dyn Fn(&mut Value)>);

const DIRECTORIES: [&str; 5] = ["effects", "effects/bundled", "tools", "tools/pawnio", "ui"];
const FILES: [(&str, &str, &[u8]); 8] = [
    ("hypercolor-app.exe", "program", b"app"),
    ("hypercolor-daemon.exe", "program", b"daemon"),
    ("hypercolor.exe", "program", b"cli"),
    ("vcruntime140.dll", "program", b"runtime"),
    ("ui/index.html", "data", b"<html></html>"),
    ("effects/bundled/rainbow.html", "data", b"rainbow"),
    ("tools/pawnio/PawnIO_setup.exe", "program", b"setup"),
    ("tools/pawnio/module.bin", "data", b"module"),
];

fn manifest() -> Value {
    let mut members = DIRECTORIES
        .iter()
        .map(|path| json!({"path": path, "type": "directory"}))
        .collect::<Vec<_>>();
    for (path, class, bytes) in FILES {
        members.push(json!({
            "path": path,
            "type": "file",
            "class": class,
            "size": bytes.len(),
            "sha256": hex::encode(Sha256::digest(bytes)),
        }));
    }
    json!({
        "name": "hypercolor",
        "version": "1.5.0",
        "platform": "windows-x86_64",
        "rust_target": "x86_64-pc-windows-msvc",
        "programs": ["hypercolor-app.exe", "hypercolor-daemon.exe", "hypercolor.exe"],
        "members": members,
        "vendor_signed": [
            {"path": "vcruntime140.dll", "vendor": "microsoft"},
            {"path": "tools/pawnio/PawnIO_setup.exe", "vendor": "pawnio"},
        ],
    })
}

fn parse(value: &Value) -> Result<ValidatedManifest, super::ReleasePayloadError> {
    ValidatedManifest::parse(serde_json::to_vec(value).expect("bytes"))
}

fn vendor_signed(value: &mut Value) -> &mut Vec<Value> {
    value["vendor_signed"].as_array_mut().expect("vendor list")
}

#[test]
fn a_valid_manifest_names_its_unit_by_digest_and_keeps_its_vendor_list() {
    let value = manifest();
    let bytes = serde_json::to_vec(&value).expect("bytes");
    let parsed = ValidatedManifest::parse(bytes.clone()).expect("valid");

    assert_eq!(parsed.unit_id.as_str(), hex::encode(Sha256::digest(&bytes)));
    assert_eq!(parsed.bytes, bytes);
    assert_eq!(
        parsed.vendor_signed.into_iter().collect::<Vec<_>>(),
        [
            (
                "tools/pawnio/PawnIO_setup.exe".to_owned(),
                WindowsVendor::Pawnio
            ),
            ("vcruntime140.dll".to_owned(), WindowsVendor::Microsoft),
        ]
    );
    assert!(matches!(
        parsed.members.get("hypercolor.exe"),
        Some(super::Member::File {
            class: PermissionClass::Program,
            ..
        })
    ));
    assert_eq!(
        parsed.children[""],
        [
            "effects",
            "hypercolor-app.exe",
            "hypercolor-daemon.exe",
            "hypercolor.exe",
            "tools",
            "ui",
            "vcruntime140.dll"
        ]
    );
}

#[test]
fn manifests_that_a_windows_unit_cannot_honour_are_refused() {
    let edits: Vec<ManifestEdit> = vec![
        (
            "linux platform",
            Box::new(|value| value["platform"] = json!("linux-x86_64")),
        ),
        (
            "gnu target",
            Box::new(|value| value["rust_target"] = json!("x86_64-pc-windows-gnu")),
        ),
        (
            "missing program",
            Box::new(|value| value["programs"] = json!(["hypercolor-app.exe"])),
        ),
        (
            "unix mode",
            Box::new(|value| value["members"][0]["mode"] = json!(493)),
        ),
        (
            "classless file",
            Box::new(|value| {
                let members = value["members"].as_array_mut().expect("members");
                let file = members
                    .iter_mut()
                    .find(|member| member["type"] == "file")
                    .expect("file");
                file.as_object_mut().expect("object").remove("class");
            }),
        ),
        (
            "classed directory",
            Box::new(|value| value["members"][0]["class"] = json!("data")),
        ),
        (
            "data program",
            Box::new(|value| {
                let members = value["members"].as_array_mut().expect("members");
                let app = members
                    .iter_mut()
                    .find(|member| member["path"] == "hypercolor-app.exe")
                    .expect("app");
                app["class"] = json!("data");
            }),
        ),
        (
            "case collision",
            Box::new(|value| {
                value["members"]
                    .as_array_mut()
                    .expect("members")
                    .push(json!({"path": "UI", "type": "directory"}));
            }),
        ),
        (
            "final and medial sigma, which NTFS merges",
            Box::new(|value| {
                let members = value["members"].as_array_mut().expect("members");
                for name in ["ui/\u{3c3}.js", "ui/\u{3c2}.js"] {
                    members.push(json!({"path": name, "type": "file", "class": "data",
                        "size": 0, "sha256": hex::encode(Sha256::digest(b""))}));
                }
            }),
        ),
        (
            "missing parent",
            Box::new(|value| {
                value["members"].as_array_mut().expect("members").push(
                    json!({"path": "nowhere/file", "type": "file", "class": "data", "size": 0,
                    "sha256": hex::encode(Sha256::digest(b""))}),
                );
            }),
        ),
        (
            "no ui",
            Box::new(|value| {
                let members = value["members"].as_array_mut().expect("members");
                members
                    .retain(|member| member["path"] != "ui" && member["path"] != "ui/index.html");
            }),
        ),
        (
            "no vendor list",
            Box::new(|value| {
                value
                    .as_object_mut()
                    .expect("object")
                    .remove("vendor_signed");
            }),
        ),
        (
            "unknown vendor",
            Box::new(|value| vendor_signed(value)[0]["vendor"] = json!("contoso")),
        ),
        (
            "vendor field beside the path",
            Box::new(|value| vendor_signed(value)[0]["publisher"] = json!("Microsoft")),
        ),
        (
            "vendor-signed data",
            Box::new(|value| {
                vendor_signed(value)
                    .push(json!({"path": "tools/pawnio/module.bin", "vendor": "pawnio"}));
            }),
        ),
        (
            "vendor-signed directory",
            Box::new(|value| {
                vendor_signed(value).push(json!({"path": "tools", "vendor": "pawnio"}));
            }),
        ),
        (
            "vendor-signed absent member",
            Box::new(|value| {
                vendor_signed(value).push(json!({"path": "tools/vcruntime140.dll",
                    "vendor": "microsoft"}));
            }),
        ),
        (
            "vendor-signed by a different case",
            Box::new(|value| {
                vendor_signed(value)[0]["path"] = json!("VCRUNTIME140.dll");
            }),
        ),
        (
            "vendor-signed twice",
            Box::new(|value| {
                vendor_signed(value).push(json!({"path": "vcruntime140.dll",
                    "vendor": "pawnio"}));
            }),
        ),
    ];
    for name in [
        "CON",
        "a:b",
        "trailing.",
        "trailing ",
        "back\\slash",
        "manifest.json",
        "MANIFEST.JSON",
        "man\u{131}fest.json",
        "",
        "a//b",
    ] {
        let mut value = manifest();
        value["members"]
            .as_array_mut()
            .expect("members")
            .push(json!({"path": name, "type": "directory"}));
        assert!(parse(&value).is_err(), "{name:?} must be refused");
    }
    for (name, edit) in edits {
        let mut value = manifest();
        edit(&mut value);
        assert!(parse(&value).is_err(), "{name} must be refused");
    }
}

#[test]
fn the_vendor_list_is_bounded() {
    let mut value = manifest();
    let members = value["members"].as_array_mut().expect("members");
    for index in 0..MAX_VENDOR_SIGNED {
        members.push(json!({
            "path": format!("tools/runtime{index}.dll"),
            "type": "file",
            "class": "program",
            "size": 0,
            "sha256": hex::encode(Sha256::digest(b"")),
        }));
    }
    let entries = (0..MAX_VENDOR_SIGNED)
        .map(|index| json!({"path": format!("tools/runtime{index}.dll"), "vendor": "microsoft"}))
        .collect::<Vec<_>>();
    value["vendor_signed"] = json!(entries);
    parse(&value).expect("the bound itself is accepted");

    vendor_signed(&mut value).push(json!({"path": "vcruntime140.dll", "vendor": "microsoft"}));
    assert!(parse(&value).is_err(), "one past the bound is refused");
}
