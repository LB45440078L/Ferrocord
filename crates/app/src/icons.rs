//! Vector icon set (Lucide, ISC license — see `assets/icons/LICENSE.txt`).
//!
//! Icons are white-stroked SVGs rasterized at the exact on-screen size and
//! tinted at draw time, so they stay crisp at any zoom and are geometrically
//! centered — unlike font glyphs, whose metrics vary between fonts.

use egui::{Color32, ImageSource, Rect, Response, Sense, Ui, Vec2};

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[allow(dead_code, reason = "full icon set; not every icon is used by every build")]
        pub enum Icon { $($variant),* }

        impl Icon {
            pub fn source(self) -> ImageSource<'static> {
                match self { $(Icon::$variant => egui::include_image!(concat!("../assets/icons/", $file, ".svg"))),* }
            }

            #[cfg(test)]
            pub const ALL: &'static [Icon] = &[$(Icon::$variant),*];
        }
    };
}

icons! {
    Plus => "plus",
    CirclePlus => "circle-plus",
    Home => "message-circle",
    Folder => "folder",
    Hash => "hash",
    Volume => "volume-2",
    VolumeOff => "volume-x",
    Megaphone => "megaphone",
    Forum => "messages-square",
    Stage => "radio",
    Thread => "corner-down-right",
    Users => "users",
    UserPlus => "user-plus",
    Pin => "pin",
    Search => "search",
    Settings => "settings",
    Close => "x",
    Smile => "smile",
    SmilePlus => "smile-plus",
    Reply => "reply",
    Pencil => "pencil",
    Trash => "trash-2",
    Check => "check",
    At => "at-sign",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    Mic => "mic",
    MicOff => "mic-off",
    Headphones => "headphones",
    HeadphonesOff => "headphone-off",
    Phone => "phone",
    PhoneOff => "phone-off",
    Video => "video",
    VideoOff => "video-off",
    File => "file",
    Film => "film",
    Music => "music",
    LogOut => "log-out",
    Copy => "copy",
    Signal => "signal",
    Maximize => "maximize-2",
    Minimize => "minimize-2",
    ArrowDown => "arrow-down",
    Shield => "shield",
}

/// Paints `icon` filling `rect` (square recommended), tinted with `color`.
pub fn paint(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    if ui.is_rect_visible(rect) {
        egui::Image::new(icon.source()).tint(color).paint_at(ui, rect);
    }
}

/// Paints `icon` of `size` centered on `center`.
pub fn paint_centered(ui: &Ui, center: egui::Pos2, size: f32, icon: Icon, color: Color32) -> Rect {
    let rect = Rect::from_center_size(center, Vec2::splat(size));
    paint(ui, rect, icon, color);
    rect
}

/// Allocates a `size`×`size` icon (non-interactive).
pub fn icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui, rect, icon, color);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_white_tintable_svg() {
        for i in Icon::ALL {
            let ImageSource::Bytes { bytes, .. } = i.source() else { panic!("{i:?} not embedded") };
            let text = std::str::from_utf8(&bytes).expect("utf8");
            assert!(text.contains("<svg"), "{i:?}");
            assert!(!text.contains("currentColor"), "{i:?} must be white-stroked for tinting");
        }
    }
}
