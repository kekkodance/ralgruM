use super::Category;

impl Category {
    pub(crate) const ALL: [Self; 4] = [Self::General, Self::Murglar, Self::Providers, Self::About];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Murglar => "Murglar",
            Self::Providers => "Providers",
            Self::About => "About",
        }
    }

    pub(crate) fn icon(self) -> crate::assets::LocalIcon {
        match self {
            Self::General => crate::assets::LocalIcon::Sliders,
            Self::Murglar => crate::assets::LocalIcon::ShieldUser,
            Self::Providers => crate::assets::LocalIcon::Server,
            Self::About => crate::assets::LocalIcon::CircleInfo,
        }
    }

    pub(crate) fn active_icon_color(self) -> u32 {
        match self {
            Self::General | Self::Murglar | Self::Providers | Self::About => {
                crate::theme::FOREGROUND
            }
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::General => 0,
            Self::Murglar => 1,
            Self::Providers => 2,
            Self::About => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Category;

    #[test]
    fn selected_settings_icons_use_foreground() {
        for category in Category::ALL {
            assert_eq!(category.active_icon_color(), crate::theme::FOREGROUND);
        }
        assert_eq!(crate::theme::FOREGROUND, 0xfafafa);
    }
}
