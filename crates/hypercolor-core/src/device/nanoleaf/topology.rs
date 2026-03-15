//! Nanoleaf panel shape metadata.

use serde::{Deserialize, Serialize};

use crate::types::device::DeviceTopologyHint;

/// Nanoleaf panel/controller shape types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum NanoleafShapeType {
    TriangleLightPanels = 0,
    Rhythm = 1,
    SquareCanvas = 2,
    ControlSquareMaster = 3,
    ControlSquarePassive = 4,
    HexagonShapes = 7,
    TriangleShapes = 8,
    MiniTriangle = 9,
    ShapesController = 12,
    ElementsHexagon = 14,
    ElementsHexCorner = 15,
    LinesConnector = 16,
    LightLines = 17,
    LightLinesSingleZone = 18,
    ControllerCap = 19,
    PowerConnector = 20,
    FourDLightstrip = 29,
    SkylightPanel = 30,
    SkylightCtrlPrimary = 31,
    SkylightCtrlPassive = 32,
}

impl NanoleafShapeType {
    /// Decode a raw Nanoleaf shape type.
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::TriangleLightPanels),
            1 => Some(Self::Rhythm),
            2 => Some(Self::SquareCanvas),
            3 => Some(Self::ControlSquareMaster),
            4 => Some(Self::ControlSquarePassive),
            7 => Some(Self::HexagonShapes),
            8 => Some(Self::TriangleShapes),
            9 => Some(Self::MiniTriangle),
            12 => Some(Self::ShapesController),
            14 => Some(Self::ElementsHexagon),
            15 => Some(Self::ElementsHexCorner),
            16 => Some(Self::LinesConnector),
            17 => Some(Self::LightLines),
            18 => Some(Self::LightLinesSingleZone),
            19 => Some(Self::ControllerCap),
            20 => Some(Self::PowerConnector),
            29 => Some(Self::FourDLightstrip),
            30 => Some(Self::SkylightPanel),
            31 => Some(Self::SkylightCtrlPrimary),
            32 => Some(Self::SkylightCtrlPassive),
            _ => None,
        }
    }

    /// Whether this shape has user-visible addressable lighting.
    #[must_use]
    pub const fn has_leds(self) -> bool {
        !matches!(
            self,
            Self::Rhythm
                | Self::ShapesController
                | Self::LinesConnector
                | Self::ControllerCap
                | Self::PowerConnector
        )
    }

    /// Approximate side length in millimeters for future layout work.
    #[must_use]
    pub const fn side_length(self) -> f64 {
        match self {
            Self::TriangleLightPanels => 150.0,
            Self::SquareCanvas | Self::ControlSquareMaster | Self::ControlSquarePassive => 100.0,
            Self::HexagonShapes | Self::MiniTriangle => 67.0,
            Self::TriangleShapes | Self::ElementsHexagon => 134.0,
            Self::ElementsHexCorner => 33.5,
            Self::LightLines => 154.0,
            Self::LightLinesSingleZone => 77.0,
            Self::FourDLightstrip => 50.0,
            Self::SkylightPanel | Self::SkylightCtrlPrimary | Self::SkylightCtrlPassive => 180.0,
            _ => 0.0,
        }
    }

    /// Map this Nanoleaf shape to Hypercolor's coarse topology hint.
    #[must_use]
    pub const fn to_topology_hint(self) -> DeviceTopologyHint {
        match self {
            Self::FourDLightstrip | Self::LightLines | Self::LightLinesSingleZone => {
                DeviceTopologyHint::Strip
            }
            _ => DeviceTopologyHint::Point,
        }
    }
}

impl From<NanoleafShapeType> for u8 {
    fn from(value: NanoleafShapeType) -> Self {
        match value {
            NanoleafShapeType::TriangleLightPanels => 0,
            NanoleafShapeType::Rhythm => 1,
            NanoleafShapeType::SquareCanvas => 2,
            NanoleafShapeType::ControlSquareMaster => 3,
            NanoleafShapeType::ControlSquarePassive => 4,
            NanoleafShapeType::HexagonShapes => 7,
            NanoleafShapeType::TriangleShapes => 8,
            NanoleafShapeType::MiniTriangle => 9,
            NanoleafShapeType::ShapesController => 12,
            NanoleafShapeType::ElementsHexagon => 14,
            NanoleafShapeType::ElementsHexCorner => 15,
            NanoleafShapeType::LinesConnector => 16,
            NanoleafShapeType::LightLines => 17,
            NanoleafShapeType::LightLinesSingleZone => 18,
            NanoleafShapeType::ControllerCap => 19,
            NanoleafShapeType::PowerConnector => 20,
            NanoleafShapeType::FourDLightstrip => 29,
            NanoleafShapeType::SkylightPanel => 30,
            NanoleafShapeType::SkylightCtrlPrimary => 31,
            NanoleafShapeType::SkylightCtrlPassive => 32,
        }
    }
}
