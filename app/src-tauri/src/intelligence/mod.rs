//! Local Intelligence is a typed, in-process translation capability. Neither
//! source nor result text is logged, persisted, emitted as an event or sent
//! over a Deck transport. Request IDs and closed errors cross the FFI.
pub(crate) mod pack;
pub(crate) mod pasteboard;
mod protected;
mod provider;
pub(crate) mod translation;
