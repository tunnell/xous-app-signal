//! Per-screen state machines. Each module contains the
//! `Screen`-variant structs for one stage's worth of screens, with
//! their `render` and `handle_key` methods. Modules group screens
//! by user-facing flow rather than per variant — `link.rs` covers
//! the four link-flow sub-states, etc.

pub mod about;
pub mod conversation_list;
pub mod empty_list;
pub mod link;
pub mod menu;
pub mod splash;
