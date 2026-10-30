//! Discord markdown: parsing ([`parse`]) and egui rendering ([`render`]).

pub mod parse;
pub mod render;

pub use parse::{Block, parse, strip_suppressed_links};
pub use render::{MdAction, MdCtx, render, twemoji_url};
