//! The see-through overlays: the voice pill, the area picker, the lens and
//! the buddy cursor. Stubs until stage 2 ports them; the blank page stands
//! in for any page path with no native page.

use super::{BlankWindow, BuddyWindow, LensWindow, RegionWindow, VoicePillWindow};

crate::simple_page!(VoicePillPage, VoicePillWindow);
crate::simple_page!(RegionPage, RegionWindow);
crate::simple_page!(LensPage, LensWindow);
crate::simple_page!(BuddyPage, BuddyWindow);
crate::simple_page!(BlankPage, BlankWindow);
