//! The Home page: hooks append events to events.jsonl, the server reads them and shows one row
//! per session. See docs/spec-home.md and docs/tla/Home.tla.
pub mod activity;
pub mod agent;
pub mod background;
pub mod clean;
pub mod events;
pub mod note;
pub mod procinfo;
pub mod read;
pub mod server;
pub mod state;
