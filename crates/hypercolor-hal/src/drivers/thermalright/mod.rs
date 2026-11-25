//! Thermalright cooler LCD panels (spec 96).
//!
//! Thermalright ships one USB identity, `0x0416:0x5302` (product string
//! `USBDISPLAY`), across several coolers whose screens differ in size and
//! pixel format. The panel says which one it is in its handshake reply, so
//! one descriptor binds the ID and the protocol picks the geometry after
//! init. This module drives the 320x240 RGB565 panels: the Frozen Warframe
//! 240, 360, and SE, and their siblings.

pub mod devices;
pub mod hid_lcd;

pub use devices::{build_hid_lcd_protocol, descriptors};
pub use hid_lcd::{
    HID_LCD_FRAME_HEADER_LEN, HID_LCD_FRAME_REPORTS, HID_LCD_KEEPALIVE_SILENCE, HID_LCD_MAGIC,
    HID_LCD_PACKET_LEN, HID_LCD_PIXEL_BYTES, HID_LCD_PUMP_INTERVAL, HID_LCD_REPORT_ID,
    HID_LCD_REPORT_LEN, HandshakeReply, HidLcdCommand, HidLcdProtocol, PanelProfile, PanelRaster,
    ReplyShape, THERMALRIGHT_HID_LCD_PID, THERMALRIGHT_VENDOR_ID, parse_handshake_reply,
};
