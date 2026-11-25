//! Thermalright cooler LCD panels (spec 96).
//!
//! Thermalright ships one USB identity, `0x0416:0x5302` (product string
//! `USBDISPLAY`), across several coolers whose screens differ in size and
//! pixel format. The panel says which one it is in its handshake reply, so
//! one descriptor binds the ID and the protocol checks the model after
//! init. This module drives the 320x240 RGB565 panels: the Frozen Warframe
//! 240, 360, and SE, and their siblings, on every firmware but 4.07.

pub mod devices;
pub mod hid_lcd;

pub use devices::THERMALRIGHT_STREAMING_FIRMWARE_RELEASE;
pub use devices::{build_hid_lcd_protocol, descriptors};
pub use hid_lcd::{
    HID_LCD_FRAME_HEADER_LEN, HID_LCD_FRAME_REPORTS, HID_LCD_HEIGHT, HID_LCD_KEEPALIVE_SILENCE,
    HID_LCD_MAGIC, HID_LCD_PACKET_LEN, HID_LCD_PIXEL_BYTES, HID_LCD_PUMP_INTERVAL,
    HID_LCD_REPORT_ID, HID_LCD_REPORT_LEN, HID_LCD_WIDTH, HandshakeReply, HidLcdCommand,
    HidLcdProtocol, THERMALRIGHT_HID_LCD_PID, THERMALRIGHT_VENDOR_ID, model_pixel_format,
    parse_handshake_reply,
};
