use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanCommand {
    Scan,
    DevStatus,
    Turn { on: bool },
    Brightness { value: u8 },
    ColorWc { red: u8, green: u8, blue: u8 },
    PtReal { command: Vec<String> },
    Razer { pt: String },
}

#[derive(Serialize)]
struct Envelope<T> {
    msg: Message<T>,
}

#[derive(Serialize)]
struct Message<T> {
    cmd: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
}

#[derive(Serialize)]
struct EmptyData {}

#[derive(Serialize)]
struct TurnData {
    value: u8,
}

#[derive(Serialize)]
struct BrightnessData {
    value: u8,
}

#[derive(Serialize)]
struct ColorWcData {
    color: RgbColor,
    #[serde(rename = "colorTemInKelvin")]
    color_tem_in_kelvin: u16,
}

#[derive(Serialize)]
struct RgbColor {
    r: u8,
    g: u8,
    b: u8,
}

#[derive(Serialize)]
struct PtRealData {
    command: Vec<String>,
}

#[derive(Serialize)]
struct RazerData {
    pt: String,
}

pub const MULTICAST_ADDR: &str = "239.255.255.250:4001";
pub const DEVICE_PORT: u16 = 4003;
pub const LISTEN_PORT: u16 = 4002;

pub fn encode_command(command: &LanCommand) -> Result<Vec<u8>> {
    match command {
        LanCommand::Scan => encode("scan", None::<EmptyData>),
        LanCommand::DevStatus => encode("devStatus", None::<EmptyData>),
        LanCommand::Turn { on } => encode(
            "turn",
            Some(TurnData {
                value: u8::from(*on),
            }),
        ),
        LanCommand::Brightness { value } => encode(
            "brightness",
            Some(BrightnessData {
                value: (*value).clamp(1, 100),
            }),
        ),
        LanCommand::ColorWc { red, green, blue } => encode(
            "colorwc",
            Some(ColorWcData {
                color: RgbColor {
                    r: *red,
                    g: *green,
                    b: *blue,
                },
                color_tem_in_kelvin: 0,
            }),
        ),
        LanCommand::PtReal { command } => encode(
            "ptReal",
            Some(PtRealData {
                command: command.clone(),
            }),
        ),
        LanCommand::Razer { pt } => encode("razer", Some(RazerData { pt: pt.clone() })),
    }
}

pub fn encode_command_string(command: &LanCommand) -> Result<String> {
    String::from_utf8(encode_command(command)?).context("Govee LAN command is not valid UTF-8")
}

fn encode<T>(cmd: &'static str, data: Option<T>) -> Result<Vec<u8>>
where
    T: Serialize,
{
    serde_json::to_vec(&Envelope {
        msg: Message { cmd, data },
    })
    .context("failed to encode Govee LAN command")
}
