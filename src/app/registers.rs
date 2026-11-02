//! Per-bus-address metadata, looked up by canonical `0xNN` string.
//!
//! Currently this just carries the publish gate and command topic. As the app
//! evolves it's the right home for the broader register lookup table that
//! feeds `consume_bus_data` (see audit M2 — zero-alloc register map).

#[derive(Debug, Clone)]
pub(crate) struct AddressMeta {
    pub(crate) mqtt_publish: bool,
    pub(crate) command_topic: Option<String>,
}
