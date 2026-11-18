//! App asset source: the default component icons plus the extra Lucide
//! icons this app uses (selected at compile time; only these SVGs are embedded).

use gpui_kit::*;
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    pub ExtraIcons,
    [
        GripVertical,
        Package,
        Compass,
        Download,
        Gamepad2,
        Layers,
        Clock,
        Rocket,
        Pin,
        PinOff,
        Trash,
        Sparkles,
        ShieldAlert,
        Swords,
        Power,
        FolderInput,
        Camera,
        ArrowUpToLine,
        ArrowDownToLine,
        Archive,
        Heart,
        User,
        Boxes,
        Combine,
        SkipForward,
        Upload,
        Wrench,
        Shuffle,
        Crosshair,
        Save,
        Link
    ]
);

pub use gpui_kit::assets::IconName as Lucide;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
