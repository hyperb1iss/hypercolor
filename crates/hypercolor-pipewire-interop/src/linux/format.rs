use std::io::Cursor;
use std::mem::size_of;

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

    serialize_object(object, "failed to serialize PipeWire format offer")
}

pub(crate) fn serialize_buffer_contract(
    format: NegotiatedVideoFormat,
) -> Result<Vec<Vec<u8>>, StreamError> {
    let stride = format
        .width
        .checked_mul(
            u32::try_from(format.format.bytes_per_pixel())
                .expect("supported packed pixel widths fit u32"),
        )
        .ok_or_else(|| contract_overflow("row stride"))?;
    let size = stride
        .checked_mul(format.height)
        .ok_or_else(|| contract_overflow("buffer size"))?;
    let stride = i32::try_from(stride).map_err(|_| contract_overflow("row stride"))?;
    let size = i32::try_from(size).map_err(|_| contract_overflow("buffer size"))?;
    let data_types = (1_u32 << spa::sys::SPA_DATA_MemPtr)
        | (1_u32 << spa::sys::SPA_DATA_MemFd)
        | (1_u32 << spa::sys::SPA_DATA_DmaBuf);
    let data_types = i32::try_from(data_types).map_err(|_| contract_overflow("data types"))?;

    let buffers = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
                    spa::utils::ChoiceFlags::empty(),
                    spa::utils::ChoiceEnum::Range {
                        default: 8,
                        min: 2,
                        max: 32,
                    },
                ))),
            ),
            spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, spa::pod::Value::Int(1)),
            spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_size, spa::pod::Value::Int(size)),
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_stride,
                spa::pod::Value::Int(stride),
            ),
            spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_align, spa::pod::Value::Int(16)),
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                spa::pod::Value::Int(data_types),
            ),
        ],
    };
    let crop = meta_object(
        spa::sys::SPA_META_VideoCrop,
        size_of::<spa::sys::spa_meta_region>(),
    )?;
    let transform = meta_object(
        spa::sys::SPA_META_VideoTransform,
        size_of::<spa::sys::spa_meta_videotransform>(),
    )?;

    [
        (buffers, "failed to serialize PipeWire buffer contract"),
        (crop, "failed to serialize PipeWire crop metadata contract"),
        (
            transform,
            "failed to serialize PipeWire transform metadata contract",
        ),
    ]
    .into_iter()
    .map(|(object, operation)| serialize_object(object, operation))
    .collect()
}

fn meta_object(meta_type: u32, size: usize) -> Result<spa::pod::Object, StreamError> {
    let size = i32::try_from(size).map_err(|_| contract_overflow("metadata size"))?;
    Ok(spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_META_type,
                spa::pod::Value::Id(spa::utils::Id(meta_type)),
            ),
            spa::pod::Property::new(spa::sys::SPA_PARAM_META_size, spa::pod::Value::Int(size)),
        ],
    })
}

fn serialize_object(
    object: spa::pod::Object,
    operation: &'static str,
) -> Result<Vec<u8>, StreamError> {
    spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|error| StreamError::Operation {
        operation,
        detail: error.to_string(),
    })
}

fn contract_overflow(field: &'static str) -> StreamError {
    StreamError::Operation {
        operation: "failed to build PipeWire buffer contract",
        detail: format!("negotiated {field} overflowed its SPA representation"),
    }
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

#[cfg(test)]
mod tests {
    use pipewire::spa;

    use super::{serialize_buffer_contract, serialize_offer};
    use crate::{
        CaptureFormatRequest, FormatOffer, NegotiatedVideoFormat, PackedVideoFormat, VideoFraction,
    };

    #[test]
    fn offer_preserves_negotiable_extent_and_transport_rate_ranges() {
        let offer = FormatOffer::new(CaptureFormatRequest {
            width: 7680,
            height: 4320,
            target_fps: 10_000,
        })
        .expect("representable high cadence and extent build");
        let bytes = serialize_offer(&offer).expect("format offer serializes");
        let (_, value) = spa::pod::deserialize::PodDeserializer::deserialize_any_from(&bytes)
            .expect("format pod deserializes to a value");
        let spa::pod::Value::Object(object) = value else {
            panic!("format pod is not an object");
        };

        let size = object
            .properties
            .iter()
            .find(|property| {
                property.key == spa::param::format::FormatProperties::VideoSize.as_raw()
            })
            .expect("format pod carries a size property");
        let spa::pod::Value::Choice(spa::pod::ChoiceValue::Rectangle(choice)) = &size.value else {
            panic!("size must be a rectangle choice so scaled outputs can fixate");
        };
        let spa::utils::Choice(_, spa::utils::ChoiceEnum::Range { default, min, max }) = choice
        else {
            panic!("size choice must be a range");
        };
        assert_eq!((default.width, default.height), (7680, 4320));
        assert_eq!((min.width, min.height), (1, 1));
        assert_eq!((max.width, max.height), (16_384, 16_384));

        let framerate = object
            .properties
            .iter()
            .find(|property| {
                property.key == spa::param::format::FormatProperties::VideoFramerate.as_raw()
            })
            .expect("format pod carries a framerate property");
        let spa::pod::Value::Choice(spa::pod::ChoiceValue::Fraction(choice)) = &framerate.value
        else {
            panic!("framerate must be a fraction choice");
        };
        let spa::utils::Choice(_, spa::utils::ChoiceEnum::Range { default, min, max }) = choice
        else {
            panic!("framerate choice must be a range");
        };
        assert_eq!((default.num, default.denom), (10_000, 1));
        assert_eq!((min.num, min.denom), (0, 1));
        assert_eq!((max.num, max.denom), (1000, 1));
    }

    #[test]
    fn fixed_format_advertises_buffers_crop_and_transform() {
        let params = serialize_buffer_contract(NegotiatedVideoFormat {
            width: 1920,
            height: 1080,
            format: PackedVideoFormat::Bgra,
            framerate: VideoFraction {
                numerator: 60,
                denominator: 1,
            },
        })
        .expect("buffer contract serializes");
        let objects = params
            .iter()
            .map(|bytes| {
                let (_, value) =
                    spa::pod::deserialize::PodDeserializer::deserialize_any_from(bytes)
                        .expect("contract pod deserializes");
                let spa::pod::Value::Object(object) = value else {
                    panic!("contract pod is not an object");
                };
                object
            })
            .collect::<Vec<_>>();

        assert_eq!(objects.len(), 3);
        assert_eq!(objects[0].id, spa::param::ParamType::Buffers.as_raw());
        let buffer_property = |key| {
            objects[0]
                .properties
                .iter()
                .find(|property| property.key == key)
                .expect("buffer property exists")
                .value
                .clone()
        };
        assert_eq!(
            buffer_property(spa::sys::SPA_PARAM_BUFFERS_blocks),
            spa::pod::Value::Int(1)
        );
        assert_eq!(
            buffer_property(spa::sys::SPA_PARAM_BUFFERS_stride),
            spa::pod::Value::Int(7680)
        );
        assert_eq!(
            buffer_property(spa::sys::SPA_PARAM_BUFFERS_size),
            spa::pod::Value::Int(8_294_400)
        );

        let meta_type = |object: &spa::pod::Object| {
            assert_eq!(object.id, spa::param::ParamType::Meta.as_raw());
            let property = object
                .properties
                .iter()
                .find(|property| property.key == spa::sys::SPA_PARAM_META_type)
                .expect("metadata type exists");
            let spa::pod::Value::Id(meta_type) = property.value else {
                panic!("metadata type is an SPA id");
            };
            meta_type.0
        };
        assert_eq!(meta_type(&objects[1]), spa::sys::SPA_META_VideoCrop);
        assert_eq!(meta_type(&objects[2]), spa::sys::SPA_META_VideoTransform);
    }
}
