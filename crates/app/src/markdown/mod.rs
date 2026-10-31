//! Discord markdown: parsing ([`mod@parse`]) and egui rendering ([`mod@render`]).

pub mod parse;
pub mod render;

pub use parse::{Block, parse, strip_suppressed_links};
pub use render::{MdAction, MdCtx, render, twemoji_url};
