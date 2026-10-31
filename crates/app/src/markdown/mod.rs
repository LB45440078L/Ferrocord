//! Discord markdown: parsing ([`mod@parse`]), egui rendering ([`mod@render`]) and
//! code block colours ([`mod@highlight`]).

pub mod highlight;
pub mod parse;
pub mod render;

pub use parse::{Block, parse, strip_suppressed_links};
pub use render::{MdAction, MdCtx, render, twemoji_url};
