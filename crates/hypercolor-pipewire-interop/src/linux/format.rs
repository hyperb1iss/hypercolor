use std::io::Cursor;

use pipewire::spa;

use crate::{
    FormatEvent, FormatFault, FormatOffer, NegotiatedVideoFormat, PackedVideoFormat, StreamError,
    VideoFraction,
};

pub(crate) fn serialize_offer(offer: &FormatOffer) -> Result<Vec<u8>, StreamError> {
    let request = offer.request;
    let object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::ARGB,
            spa::param::video::VideoFormat::ABGR,
            spa::param::video::VideoFormat::xRGB,
            spa::param::video::VideoFormat::xBGR,
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: request.width,
                height: request.height,
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1,
            },
            spa::utils::Rectangle {
                width: 16_384,
                height: 16_384,
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction {
                num: request.target_fps,
                denom: 1,
            },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1,
            }
        ),
    );

    spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|error| StreamError::Operation {
        operation: "failed to serialize PipeWire format offer",
        detail: error.to_string(),
    })
}

pub(crate) fn parse_event(param: Option<&spa::pod::Pod>) -> FormatEvent {
    let Some(param) = param else {
        return FormatEvent::Removed;
    };
    let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param) else {
        return FormatEvent::Invalid(FormatFault::Unreadable);
    };
    if media_type != spa::param::format::MediaType::Video
        || media_subtype != spa::param::format::MediaSubtype::Raw
    {
        return FormatEvent::Invalid(FormatFault::NonRawVideo);
    }

    let mut info = spa::param::video::VideoInfoRaw::new();
    if info.parse(param).is_err() {
        return FormatEvent::Invalid(FormatFault::InvalidRawVideo);
    }
    let Some(format) = packed_video_format(info.format()) else {
        return FormatEvent::Invalid(FormatFault::UnsupportedPixelFormat);
    };
    let size = info.size();
    let frame_rate = info.framerate();
    FormatEvent::Negotiated(NegotiatedVideoFormat {
        width: size.width,
        height: size.height,
        format,
        framerate: VideoFraction {
            numerator: frame_rate.num,
            denominator: frame_rate.denom,
        },
    })
}

fn packed_video_format(format: spa::param::video::VideoFormat) -> Option<PackedVideoFormat> {
    match format {
        spa::param::video::VideoFormat::RGBA => Some(PackedVideoFormat::Rgba),
        spa::param::video::VideoFormat::BGRA => Some(PackedVideoFormat::Bgra),
        spa::param::video::VideoFormat::RGBx => Some(PackedVideoFormat::Rgbx),
        spa::param::video::VideoFormat::BGRx => Some(PackedVideoFormat::Bgrx),
        spa::param::video::VideoFormat::ARGB => Some(PackedVideoFormat::Argb),
        spa::param::video::VideoFormat::ABGR => Some(PackedVideoFormat::Abgr),
        spa::param::video::VideoFormat::xRGB => Some(PackedVideoFormat::Xrgb),
        spa::param::video::VideoFormat::xBGR => Some(PackedVideoFormat::Xbgr),
        spa::param::video::VideoFormat::RGB => Some(PackedVideoFormat::Rgb),
        spa::param::video::VideoFormat::BGR => Some(PackedVideoFormat::Bgr),
        _ => None,
    }
}
