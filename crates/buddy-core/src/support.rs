use serde::Serialize;

/// The approved OS support floors. These values are pinned against drift by
/// the `support_floors_match_spec` integration test.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SupportFloors {
    pub windows_min: &'static str,
    pub macos_min: &'static str,
    pub linux_requires_pipewire: bool,
}

pub const SUPPORT_FLOORS: SupportFloors = SupportFloors {
    windows_min: "10 version 1703",
    macos_min: "14.2",
    linux_requires_pipewire: true,
};
