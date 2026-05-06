//! Per-screen state machines. Each module contains exactly one
//! `Screen`-variant struct, its `render` method, and its
//! `handle_key` method. ≤ 100 LoC per screen by design.

pub mod about;
pub mod empty_list;
pub mod menu;
pub mod splash;
