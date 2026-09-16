use std::{borrow::Cow, collections::BTreeSet};

use gpui::{AssetSource, SharedString, Styled, rgb, svg};
use rust_embed::RustEmbed;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalIcon {
    Music,
    QuoteRight,
    Compass,
    Layers,
    Download,
    Upload,
    FileImport,
    FolderOpen,
    User,
    Headphones,
    Settings,
    ShieldUser,
    UserLock,
    Lock,
    LogIn,
    LogOut,
    Key,
    Server,
    X,
    Pen,
    Check,
    Deezer,
    SoundCloud,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    MagnifyingGlass,
    EarthAmericas,
    List,
    ListCheck,
    ShareNodes,
    CompactDisc,
    Image,
    UserGroup,
    ListUl,
    TriangleExclamation,
    Minus,
    WindowMaximize,
    WindowRestore,
    Heart,
    ClockRotateLeft,
    CloudArrowUp,
    Radio,
    Plus,
    Play,
    Pause,
    Previous,
    Next,
    ForwardStep,
    VolumeHigh,
    VolumeLow,
    VolumeOff,
    VolumeMuted,
    Shuffle,
    Repeat,
    CircleInfo,
    CircleCheck,
    CircleXmark,
    TrashCan,
    EllipsisVertical,
    ChevronUp,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Eye,
    EyeSlash,
    Spinner,
    Brain,
    Sliders,
    HardDrive,
    Signal,
    RotateRight,
    IdCard,
    Scissors,
    Copy,
    Paste,
    ObjectGroup,
    CreditCard,
    Infinity,
    Telegram,
    PayPal,
    Bitcoin,
    GitHub,
}

impl LocalIcon {
    #[cfg(test)]
    pub(crate) const ALL: [LocalIcon; 84] = [
        LocalIcon::Music,
        LocalIcon::QuoteRight,
        LocalIcon::Compass,
        LocalIcon::Layers,
        LocalIcon::Download,
        LocalIcon::Upload,
        LocalIcon::FileImport,
        LocalIcon::FolderOpen,
        LocalIcon::User,
        LocalIcon::Headphones,
        LocalIcon::Settings,
        LocalIcon::ShieldUser,
        LocalIcon::UserLock,
        LocalIcon::Lock,
        LocalIcon::LogIn,
        LocalIcon::LogOut,
        LocalIcon::Key,
        LocalIcon::Server,
        LocalIcon::X,
        LocalIcon::Pen,
        LocalIcon::Check,
        LocalIcon::Deezer,
        LocalIcon::SoundCloud,
        LocalIcon::ArrowLeft,
        LocalIcon::ArrowRight,
        LocalIcon::ArrowUp,
        LocalIcon::ArrowDown,
        LocalIcon::MagnifyingGlass,
        LocalIcon::EarthAmericas,
        LocalIcon::List,
        LocalIcon::ListCheck,
        LocalIcon::ShareNodes,
        LocalIcon::CompactDisc,
        LocalIcon::Image,
        LocalIcon::UserGroup,
        LocalIcon::ListUl,
        LocalIcon::TriangleExclamation,
        LocalIcon::Minus,
        LocalIcon::WindowMaximize,
        LocalIcon::WindowRestore,
        LocalIcon::Heart,
        LocalIcon::ClockRotateLeft,
        LocalIcon::CloudArrowUp,
        LocalIcon::Radio,
        LocalIcon::Plus,
        LocalIcon::Play,
        LocalIcon::Pause,
        LocalIcon::Previous,
        LocalIcon::Next,
        LocalIcon::ForwardStep,
        LocalIcon::VolumeHigh,
        LocalIcon::VolumeLow,
        LocalIcon::VolumeOff,
        LocalIcon::VolumeMuted,
        LocalIcon::Shuffle,
        LocalIcon::Repeat,
        LocalIcon::CircleInfo,
        LocalIcon::CircleCheck,
        LocalIcon::CircleXmark,
        LocalIcon::TrashCan,
        LocalIcon::EllipsisVertical,
        LocalIcon::ChevronUp,
        LocalIcon::ChevronDown,
        LocalIcon::ChevronLeft,
        LocalIcon::ChevronRight,
        LocalIcon::Eye,
        LocalIcon::EyeSlash,
        LocalIcon::Spinner,
        LocalIcon::Brain,
        LocalIcon::Sliders,
        LocalIcon::HardDrive,
        LocalIcon::Signal,
        LocalIcon::RotateRight,
        LocalIcon::IdCard,
        LocalIcon::Scissors,
        LocalIcon::Copy,
        LocalIcon::Paste,
        LocalIcon::ObjectGroup,
        LocalIcon::CreditCard,
        LocalIcon::Infinity,
        LocalIcon::Telegram,
        LocalIcon::PayPal,
        LocalIcon::Bitcoin,
        LocalIcon::GitHub,
    ];

    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Music => "ralgrum/icons/fontawesome-free-7.3.1/solid/music.svg",
            Self::QuoteRight => "ralgrum/icons/fontawesome-free-7.3.1/solid/quote-right.svg",
            Self::Compass => "ralgrum/icons/fontawesome-free-7.3.1/solid/compass.svg",
            Self::Layers => "ralgrum/icons/fontawesome-free-7.3.1/solid/layer-group.svg",
            Self::Download => "ralgrum/icons/fontawesome-free-7.3.1/solid/download.svg",
            Self::Upload => "ralgrum/icons/fontawesome-free-7.3.1/solid/upload.svg",
            Self::FileImport => "ralgrum/icons/fontawesome-free-7.3.1/solid/file-import.svg",
            Self::FolderOpen => "ralgrum/icons/fontawesome-free-7.3.1/solid/folder-open.svg",
            Self::User => "ralgrum/icons/fontawesome-free-7.3.1/solid/user.svg",
            Self::Headphones => "ralgrum/icons/fontawesome-free-7.3.1/solid/headphones-simple.svg",
            Self::Settings => "ralgrum/icons/fontawesome-free-7.3.1/solid/gear.svg",
            Self::ShieldUser => "ralgrum/icons/fontawesome-free-7.3.1/solid/user-shield.svg",
            Self::UserLock => "ralgrum/icons/fontawesome-free-7.3.1/solid/user-lock.svg",
            Self::Lock => "ralgrum/icons/fontawesome-free-7.3.1/solid/lock.svg",
            Self::LogIn => "ralgrum/icons/fontawesome-free-7.3.1/solid/right-to-bracket.svg",
            Self::LogOut => "ralgrum/icons/fontawesome-free-7.3.1/solid/right-from-bracket.svg",
            Self::Key => "ralgrum/icons/fontawesome-free-7.3.1/solid/key.svg",
            Self::Server => "ralgrum/icons/fontawesome-free-7.3.1/solid/server.svg",
            Self::X => "ralgrum/icons/fontawesome-free-7.3.1/solid/xmark.svg",
            Self::Pen => "ralgrum/icons/fontawesome-free-7.3.1/solid/pen.svg",
            Self::Check => "ralgrum/icons/fontawesome-free-7.3.1/solid/check.svg",
            Self::Deezer => "ralgrum/icons/fontawesome-free-7.3.1/brands/deezer.svg",
            Self::SoundCloud => "ralgrum/icons/fontawesome-free-7.3.1/brands/soundcloud.svg",
            Self::ArrowLeft => "ralgrum/icons/fontawesome-free-7.3.1/solid/arrow-left.svg",
            Self::ArrowRight => "ralgrum/icons/fontawesome-free-7.3.1/solid/arrow-right.svg",
            Self::ArrowUp => "ralgrum/icons/fontawesome-free-7.3.1/solid/arrow-up.svg",
            Self::ArrowDown => "ralgrum/icons/fontawesome-free-7.3.1/solid/arrow-down.svg",
            Self::MagnifyingGlass => {
                "ralgrum/icons/fontawesome-free-7.3.1/solid/magnifying-glass.svg"
            }
            Self::EarthAmericas => "ralgrum/icons/fontawesome-free-7.3.1/solid/earth-americas.svg",
            Self::List => "ralgrum/icons/fontawesome-free-7.3.1/solid/list.svg",
            Self::ListCheck => "ralgrum/icons/fontawesome-free-7.3.1/solid/list-check.svg",
            Self::ShareNodes => "ralgrum/icons/fontawesome-free-7.3.1/solid/share-nodes.svg",
            Self::CompactDisc => "ralgrum/icons/fontawesome-free-7.3.1/solid/compact-disc.svg",
            Self::Image => "ralgrum/icons/fontawesome-free-7.3.1/regular/image.svg",
            Self::UserGroup => "ralgrum/icons/fontawesome-free-7.3.1/solid/user-group.svg",
            Self::ListUl => "ralgrum/icons/fontawesome-free-7.3.1/solid/list-ul.svg",
            Self::TriangleExclamation => {
                "ralgrum/icons/fontawesome-free-7.3.1/solid/triangle-exclamation.svg"
            }
            Self::Minus => "ralgrum/icons/fontawesome-free-7.3.1/solid/minus.svg",
            Self::WindowMaximize => {
                "ralgrum/icons/fontawesome-free-7.3.1/regular/window-maximize.svg"
            }
            Self::WindowRestore => {
                "ralgrum/icons/fontawesome-free-7.3.1/regular/window-restore.svg"
            }
            Self::Heart => "ralgrum/icons/fontawesome-free-7.3.1/solid/heart.svg",
            Self::ClockRotateLeft => {
                "ralgrum/icons/fontawesome-free-7.3.1/solid/clock-rotate-left.svg"
            }
            Self::CloudArrowUp => "ralgrum/icons/fontawesome-free-7.3.1/solid/cloud-arrow-up.svg",
            Self::Radio => "ralgrum/icons/fontawesome-free-7.3.1/solid/radio.svg",
            Self::Plus => "ralgrum/icons/fontawesome-free-7.3.1/solid/plus.svg",
            Self::Play => "ralgrum/icons/fontawesome-free-7.3.1/solid/play.svg",
            Self::Pause => "ralgrum/icons/fontawesome-free-7.3.1/solid/pause.svg",
            Self::Previous => "ralgrum/icons/fontawesome-free-7.3.1/solid/backward-step.svg",
            Self::Next => "ralgrum/icons/fontawesome-free-7.3.1/solid/forward-step.svg",
            Self::ForwardStep => "ralgrum/icons/fontawesome-free-7.3.1/solid/forward-step.svg",
            Self::VolumeHigh => "ralgrum/icons/fontawesome-free-7.3.1/solid/volume-high.svg",
            Self::VolumeLow => "ralgrum/icons/fontawesome-free-7.3.1/solid/volume-low.svg",
            Self::VolumeOff => "ralgrum/icons/fontawesome-free-7.3.1/solid/volume-off.svg",
            Self::VolumeMuted => "ralgrum/icons/fontawesome-free-7.3.1/solid/volume-xmark.svg",
            Self::Shuffle => "ralgrum/icons/fontawesome-free-7.3.1/solid/shuffle.svg",
            Self::Repeat => "ralgrum/icons/fontawesome-free-7.3.1/solid/repeat.svg",
            Self::CircleInfo => "ralgrum/icons/fontawesome-free-7.3.1/solid/circle-info.svg",
            Self::CircleCheck => "ralgrum/icons/fontawesome-free-7.3.1/solid/circle-check.svg",
            Self::CircleXmark => "ralgrum/icons/fontawesome-free-7.3.1/solid/circle-xmark.svg",
            Self::TrashCan => "ralgrum/icons/fontawesome-free-7.3.1/solid/trash-can.svg",
            Self::EllipsisVertical => {
                "ralgrum/icons/fontawesome-free-7.3.1/solid/ellipsis-vertical.svg"
            }
            Self::ChevronUp => "ralgrum/icons/fontawesome-free-7.3.1/solid/chevron-up.svg",
            Self::ChevronDown => "ralgrum/icons/fontawesome-free-7.3.1/solid/chevron-down.svg",
            Self::ChevronLeft => "ralgrum/icons/fontawesome-free-7.3.1/solid/chevron-left.svg",
            Self::ChevronRight => "ralgrum/icons/fontawesome-free-7.3.1/solid/chevron-right.svg",
            Self::Eye => "ralgrum/icons/fontawesome-free-7.3.1/solid/eye.svg",
            Self::EyeSlash => "ralgrum/icons/fontawesome-free-7.3.1/solid/eye-slash.svg",
            Self::Spinner => "ralgrum/icons/fontawesome-free-7.3.1/solid/spinner.svg",
            Self::Brain => "ralgrum/icons/fontawesome-free-7.3.1/solid/brain.svg",
            Self::Sliders => "ralgrum/icons/fontawesome-free-7.3.1/solid/sliders.svg",
            Self::HardDrive => "ralgrum/icons/fontawesome-free-7.3.1/solid/hard-drive.svg",
            Self::Signal => "ralgrum/icons/fontawesome-free-7.3.1/solid/signal.svg",
            Self::RotateRight => "ralgrum/icons/fontawesome-free-7.3.1/solid/rotate-right.svg",
            Self::IdCard => "ralgrum/icons/fontawesome-free-7.3.1/solid/id-card.svg",
            Self::Scissors => "ralgrum/icons/fontawesome-free-7.3.1/solid/scissors.svg",
            Self::Copy => "ralgrum/icons/fontawesome-free-7.3.1/regular/copy.svg",
            Self::Paste => "ralgrum/icons/fontawesome-free-7.3.1/solid/paste.svg",
            Self::ObjectGroup => "ralgrum/icons/fontawesome-free-7.3.1/solid/object-group.svg",
            Self::CreditCard => "ralgrum/icons/fontawesome-free-7.3.1/solid/credit-card.svg",
            Self::Infinity => "ralgrum/icons/fontawesome-free-7.3.1/solid/infinity.svg",
            Self::Telegram => "ralgrum/icons/fontawesome-free-7.3.1/brands/telegram.svg",
            Self::PayPal => "ralgrum/icons/fontawesome-free-7.3.1/brands/paypal.svg",
            Self::Bitcoin => "ralgrum/icons/fontawesome-free-7.3.1/brands/bitcoin.svg",
            Self::GitHub => "ralgrum/icons/fontawesome-free-7.3.1/brands/github.svg",
        }
    }
}

pub(crate) fn local_icon(icon: LocalIcon, color: u32) -> gpui::Svg {
    svg().path(icon.path()).text_color(rgb(color))
}

pub(crate) fn widget_icon(icon: LocalIcon) -> gpui_component::Icon {
    gpui_component::Icon::default().path(icon.path())
}

fn font_awesome_file_for_widget_icon(path: &str) -> Option<&'static str> {
    let name = path.strip_prefix("icons/")?.strip_suffix(".svg")?;
    let icon = match name {
        "arrow-left" => LocalIcon::ArrowLeft,
        "arrow-down" => LocalIcon::ArrowDown,
        "arrow-up" => LocalIcon::ArrowUp,
        "arrow-right" => LocalIcon::ArrowRight,
        "check" => LocalIcon::Check,
        "chevron-down" => LocalIcon::ChevronDown,
        "chevron-left" => LocalIcon::ChevronLeft,
        "chevron-right" => LocalIcon::ChevronRight,
        "chevron-up" => LocalIcon::ChevronUp,
        "circle-check" => LocalIcon::CircleCheck,
        "circle-x" => LocalIcon::CircleXmark,
        "close" | "window-close" => LocalIcon::X,
        "copy" => LocalIcon::Copy,
        "ellipsis-vertical" => LocalIcon::EllipsisVertical,
        "external-link" => LocalIcon::ArrowRight,
        "eye" => LocalIcon::Eye,
        "eye-off" => LocalIcon::EyeSlash,
        "file-import" => LocalIcon::FileImport,
        "folder-open" | "inbox" => LocalIcon::FolderOpen,
        "github" => LocalIcon::GitHub,
        "globe" => LocalIcon::EarthAmericas,
        "hard-drive" => LocalIcon::HardDrive,
        "heart" => LocalIcon::Heart,
        "info" => LocalIcon::CircleInfo,
        "loader" | "loader-circle" => LocalIcon::Spinner,
        "minus" => LocalIcon::Minus,
        "pause" => LocalIcon::Pause,
        "play" => LocalIcon::Play,
        "plus" => LocalIcon::Plus,
        "search" => LocalIcon::MagnifyingGlass,
        "settings" | "settings-2" => LocalIcon::Settings,
        "triangle-alert" => LocalIcon::TriangleExclamation,
        "upload" => LocalIcon::Upload,
        "user" => LocalIcon::User,
        "window-maximize" => LocalIcon::WindowMaximize,
        "window-restore" => LocalIcon::WindowRestore,
        _ => return None,
    };
    icon.path().strip_prefix("ralgrum/")
}

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/fontawesome-free-7.3.1/**/*.svg"]
#[include = "app-icon.png"]
struct LocalAssets;

pub(crate) struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if path.starts_with("ralgrum/") {
            let physical_path = path.strip_prefix("ralgrum/").unwrap_or(path);
            return load_local_asset(path, physical_path);
        }
        if let Some(physical_path) = font_awesome_file_for_widget_icon(path) {
            return load_local_asset(path, physical_path);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("could not find asset at path \"{path}\""),
        )
        .into())
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = BTreeSet::new();
        let physical_prefix = if path.is_empty() || path == "ralgrum" {
            Some("")
        } else {
            path.strip_prefix("ralgrum/")
        };

        if let Some(physical_prefix) = physical_prefix {
            paths.extend(
                LocalAssets::iter()
                    .filter(|item| item.starts_with(physical_prefix))
                    .map(|item| SharedString::from(format!("ralgrum/{item}"))),
            );
        }
        Ok(paths.into_iter().collect())
    }
}

fn load_local_asset(
    logical_path: &str,
    physical_path: &str,
) -> gpui::Result<Option<Cow<'static, [u8]>>> {
    LocalAssets::get(physical_path)
        .map(|asset| Some(asset.data))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("could not find asset at path \"{logical_path}\""),
            )
            .into()
        })
}

#[cfg(test)]
mod tests {
    use super::{AppAssets, LocalAssets, LocalIcon};
    use gpui::AssetSource;

    #[test]
    fn every_local_icon_is_served_by_app_assets() {
        for icon in LocalIcon::ALL {
            let physical_path = icon.path().strip_prefix("ralgrum/").unwrap_or(icon.path());
            let local =
                LocalAssets::get(physical_path).unwrap_or_else(|| panic!("{}", icon.path()));
            let loaded = AppAssets
                .load(icon.path())
                .unwrap_or_else(|error| panic!("{}: {error}", icon.path()))
                .unwrap_or_else(|| panic!("{}", icon.path()));

            assert_eq!(loaded.as_ref(), local.data.as_ref(), "{}", icon.path());
        }

        assert_eq!(
            LocalIcon::ShieldUser.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/user-shield.svg"
        );
        assert_eq!(
            LocalIcon::LogIn.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/right-to-bracket.svg"
        );
        assert_eq!(
            LocalIcon::LogOut.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/right-from-bracket.svg"
        );
        assert_eq!(
            LocalIcon::Key.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/key.svg"
        );
        assert_eq!(
            LocalIcon::Server.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/server.svg"
        );
        assert_eq!(
            LocalIcon::Next.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/forward-step.svg"
        );
        assert_eq!(
            LocalIcon::Scissors.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/scissors.svg"
        );
        assert_eq!(
            LocalIcon::Copy.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/regular/copy.svg"
        );
        assert_eq!(
            LocalIcon::Paste.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/paste.svg"
        );
        assert_eq!(
            LocalIcon::ObjectGroup.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/object-group.svg"
        );
        assert_eq!(
            LocalIcon::ForwardStep.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/forward-step.svg"
        );
        assert_eq!(
            LocalIcon::QuoteRight.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/quote-right.svg"
        );
        assert_eq!(
            LocalIcon::Check.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/check.svg"
        );
        assert_eq!(
            LocalIcon::Deezer.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/brands/deezer.svg"
        );
        assert_eq!(
            LocalIcon::SoundCloud.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/brands/soundcloud.svg"
        );
        assert_eq!(
            LocalIcon::WindowMaximize.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/regular/window-maximize.svg"
        );
        assert_eq!(
            LocalIcon::WindowRestore.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/regular/window-restore.svg"
        );
        assert_eq!(
            LocalIcon::Infinity.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/infinity.svg"
        );
        assert_eq!(
            LocalIcon::ListCheck.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/list-check.svg"
        );
        assert_eq!(
            LocalIcon::ShareNodes.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/share-nodes.svg"
        );
        assert_eq!(
            LocalIcon::GitHub.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/brands/github.svg"
        );
    }

    #[test]
    fn hard_drive_icon_uses_the_supplied_font_awesome_shape() {
        let asset = LocalAssets::get("icons/fontawesome-free-7.3.1/solid/hard-drive.svg")
            .expect("embedded hard-drive icon");
        let svg = std::str::from_utf8(&asset.data).expect("hard-drive SVG text");

        assert!(svg.contains("viewBox=\"0 0 448 512\""));
        assert!(svg.contains("d=\"M64 32C28.7 32 0 60.7 0 96"));
        assert!(svg.contains("fill=\"currentColor\""));
    }

    #[test]
    fn upload_icon_uses_the_supplied_font_awesome_shape_and_current_color() {
        let asset = LocalAssets::get("icons/fontawesome-free-7.3.1/solid/upload.svg")
            .expect("embedded upload icon");
        let svg = std::str::from_utf8(&asset.data).expect("upload SVG text");

        assert_eq!(
            LocalIcon::Upload.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/upload.svg"
        );
        assert!(svg.contains("Font Awesome Free 7.3.1"));
        assert!(svg.contains("fill=\"currentColor\""));
        assert!(
            AppAssets
                .load("icons/upload.svg")
                .expect("widget upload icon")
                .is_some()
        );
    }

    #[test]
    fn file_import_icon_uses_the_supplied_font_awesome_shape_and_current_color() {
        let asset = LocalAssets::get("icons/fontawesome-free-7.3.1/solid/file-import.svg")
            .expect("embedded file-import icon");
        let svg = std::str::from_utf8(&asset.data).expect("file-import SVG text");

        assert_eq!(
            LocalIcon::FileImport.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/file-import.svg"
        );
        assert!(svg.contains("Font Awesome Free 7.3.1"));
        assert!(svg.contains("fill=\"currentColor\""));
        assert!(
            AppAssets
                .load("icons/file-import.svg")
                .expect("widget file-import icon")
                .is_some()
        );
    }

    #[test]
    fn app_icon_is_served_by_app_assets() {
        let path = "ralgrum/app-icon.png";
        let local = LocalAssets::get("app-icon.png").expect("embedded app-icon.png");
        let loaded = AppAssets
            .load(path)
            .unwrap_or_else(|error| panic!("{path}: {error}"))
            .unwrap_or_else(|| panic!("{path}"));

        assert_eq!(loaded.as_ref(), local.data.as_ref());
        assert!(!loaded.is_empty());
    }

    #[test]
    fn local_asset_listing_keeps_the_logical_namespace() {
        let prefix = "ralgrum/icons/fontawesome-free-7.3.1/solid/";
        let paths = AppAssets.list(prefix).expect("logical local asset list");

        assert!(
            paths
                .iter()
                .any(|path| path.as_ref() == LocalIcon::Music.path())
        );
        assert!(
            paths
                .iter()
                .all(|path| path.as_ref().starts_with("ralgrum/"))
        );
    }

    #[test]
    fn physical_local_asset_paths_are_not_listed_as_app_paths() {
        let physical_prefix = "icons/fontawesome-free-7.3.1/solid/";
        let paths = AppAssets
            .list(physical_prefix)
            .expect("component asset list for physical local prefix");

        assert!(
            paths
                .iter()
                .all(|path| !path.as_ref().starts_with("ralgrum/"))
        );
    }

    #[test]
    fn widget_icon_paths_serve_font_awesome_instead_of_lucide() {
        let loaded = AppAssets
            .load("icons/check.svg")
            .expect("widget check icon")
            .expect("widget check icon bytes");
        let fa = LocalAssets::get("icons/fontawesome-free-7.3.1/solid/check.svg")
            .expect("font awesome check");
        assert_eq!(loaded.as_ref(), fa.data.as_ref());
        let as_text = std::str::from_utf8(&loaded).expect("svg text");
        assert!(as_text.contains("Font Awesome"));
        assert!(!as_text.contains("lucide"));
    }
}
