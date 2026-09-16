use std::sync::Arc;

use gpui::{Context, Entity};
use tokio::runtime::Runtime;

use crate::settings::AccountState;

use super::{
    favorite_state::{FavoriteKey, FavoriteKind},
    model::{Category, Page},
    view::LibraryView,
};

pub(crate) struct FavoriteController {
    account: Entity<AccountState>,
    favorites: Entity<super::favorite_state::FavoriteState>,
    runtime: Arc<Runtime>,
    client: Result<super::client::LibraryClient, String>,
    soundcloud_client: Result<super::soundcloud_client::SoundCloudLibraryClient, String>,
}

impl FavoriteController {
    pub(crate) fn new(
        account: Entity<AccountState>,
        favorites: Entity<super::favorite_state::FavoriteState>,
        runtime: Arc<Runtime>,
    ) -> Self {
        Self {
            account,
            favorites,
            runtime,
            client: super::client::LibraryClient::new(),
            soundcloud_client: super::soundcloud_client::SoundCloudLibraryClient::new(),
        }
    }

    pub(crate) fn toggle(
        &mut self,
        key: FavoriteKey,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(mutation) = self
            .favorites
            .update(cx, |favorites, _| favorites.begin(key, known_favorite))
        else {
            return;
        };
        let request = mutation.clone();
        let task = match request.key.provider {
            crate::search::Provider::Deezer => {
                let Some(arl) = self.account.read(cx).deezer_arl() else {
                    self.reject(&mutation, "Deezer account required", cx);
                    return;
                };
                let saved_user_id = self.account.read(cx).deezer_user_id();
                let Ok(client) = self.client.clone() else {
                    self.reject(&mutation, "Favorite client could not be created", cx);
                    return;
                };
                self.runtime.spawn(async move {
                    client
                        .set_favorite(request.key, request.favorite, arl, saved_user_id)
                        .await
                })
            }
            crate::search::Provider::SoundCloud => {
                let (token, session_cookies) = {
                    let account = self.account.read(cx);
                    (
                        account.soundcloud_mobile_token(),
                        account.soundcloud_session_cookies(),
                    )
                };
                let Some(token) = token else {
                    self.reject(&mutation, "SoundCloud account required", cx);
                    return;
                };
                let Some(session_cookies) = session_cookies else {
                    self.reject(&mutation, "SoundCloud account required", cx);
                    return;
                };
                let Ok(client) = self.soundcloud_client.clone() else {
                    self.reject(&mutation, "Favorite client could not be created", cx);
                    return;
                };
                self.runtime.spawn(async move {
                    client
                        .set_favorite(request.key, request.favorite, token, session_cookies)
                        .await
                })
            }
        };
        let favorites = self.favorites.clone();
        cx.spawn(async move |_, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Favorite request failed".into()));
            favorites.update(cx, |favorites, cx| {
                if let Err(ref error) = result {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Favorite failed",
                        Some(error.clone().into()),
                    );
                }
                if favorites.complete(&mutation, result) {
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn reject(
        &self,
        mutation: &super::favorite_state::FavoriteMutation,
        error: &str,
        cx: &mut Context<Self>,
    ) {
        self.favorites.update(cx, |favorites, _| {
            favorites.complete(mutation, Err(error.to_owned()));
        });
        crate::toast::push_global(
            cx,
            crate::toast::ToastKind::Error,
            "Favorite failed",
            Some(error.to_owned().into()),
        );
    }
}

pub(crate) fn category_for(kind: FavoriteKind) -> Category {
    match kind {
        FavoriteKind::Track => Category::Tracks,
        FavoriteKind::Album => Category::Albums,
        FavoriteKind::Artist => Category::Artists,
        FavoriteKind::Playlist => Category::Playlists,
    }
}

fn page_contains_favorite(page: &Page, kind: FavoriteKind, id: &str) -> bool {
    match kind {
        FavoriteKind::Track => page.tracks.iter().any(|track| track.id == id),
        FavoriteKind::Album | FavoriteKind::Artist | FavoriteKind::Playlist => {
            page.cards.iter().any(|card| card.id == id)
        }
    }
}

impl LibraryView {
    pub(crate) fn resolve_favorite_state(&mut self, key: FavoriteKey, cx: &mut Context<Self>) {
        if key.provider == crate::search::Provider::Deezer {
            let generation = self.favorites.update(cx, |favorites, _| {
                if !favorites.begin_resolve(key.clone()) {
                    return None;
                }
                favorites.begin_catalog_load(key.provider, key.kind)
            });
            let Some(generation) = generation else {
                cx.notify();
                return;
            };
            let Some(arl) = self.account.read(cx).deezer_arl() else {
                self.favorites.update(cx, |favorites, _| {
                    favorites.finish_catalog_load(key.provider, key.kind, generation, None);
                });
                cx.notify();
                return;
            };
            let saved_user_id = self.account.read(cx).deezer_user_id();
            let Ok(client) = self.client.clone() else {
                self.favorites.update(cx, |favorites, _| {
                    favorites.finish_catalog_load(key.provider, key.kind, generation, None);
                });
                cx.notify();
                return;
            };
            let kind = key.kind;
            let task = self
                .runtime
                .spawn(async move { client.load_favorite_catalog(arl, saved_user_id, kind).await });
            self.set_favorite_catalog_cancel(kind, generation, task.abort_handle());
            let favorites = self.favorites.clone();
            cx.spawn(async move |this, cx| {
                let ids = task.await.ok().and_then(Result::ok);
                this.update(cx, |this, cx| {
                    this.clear_favorite_catalog_cancel(kind, generation);
                    favorites.update(cx, |favorites, _| {
                        favorites.finish_catalog_load(
                            crate::search::Provider::Deezer,
                            kind,
                            generation,
                            ids,
                        );
                    });
                    cx.notify();
                })
                .ok();
            })
            .detach();
            cx.notify();
            return;
        }

        let started = self
            .favorites
            .update(cx, |favorites, _| favorites.begin_resolve(key.clone()));
        if !started {
            return;
        }

        let category = category_for(key.kind);
        let task = match key.provider {
            crate::search::Provider::Deezer => {
                let Some(arl) = self.account.read(cx).deezer_arl() else {
                    self.favorites
                        .update(cx, |favorites, _| favorites.finish_resolve(&key, None));
                    return;
                };
                let saved_user_id = self.account.read(cx).deezer_user_id();
                let Ok(client) = self.client.clone() else {
                    self.favorites
                        .update(cx, |favorites, _| favorites.finish_resolve(&key, None));
                    return;
                };
                self.runtime
                    .spawn(async move { client.load(category, arl, saved_user_id).await })
            }
            crate::search::Provider::SoundCloud => {
                let Some(token) = self.account.read(cx).soundcloud_mobile_token() else {
                    self.favorites
                        .update(cx, |favorites, _| favorites.finish_resolve(&key, None));
                    return;
                };
                let Ok(client) = self.soundcloud_library_client() else {
                    self.favorites
                        .update(cx, |favorites, _| favorites.finish_resolve(&key, None));
                    return;
                };
                self.runtime
                    .spawn(async move { client.load(category, token).await })
            }
        };

        let favorites = self.favorites.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await.ok().and_then(Result::ok);
            let resolved = result
                .as_ref()
                .map(|page| page_contains_favorite(page, key.kind, &key.id));
            favorites.update(cx, |favorites, _| {
                favorites.finish_resolve(&key, resolved);
            });
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    pub(crate) fn toggle_favorite(
        &mut self,
        key: FavoriteKey,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(mutation) = self
            .favorites
            .update(cx, |favorites, _| favorites.begin(key, known_favorite))
        else {
            return;
        };
        let request = mutation.clone();
        let task = match request.key.provider {
            crate::search::Provider::Deezer => {
                let Some(arl) = self.account.read(cx).deezer_arl() else {
                    self.finish_favorite(&mutation, Err("Deezer account required".into()), cx);
                    return;
                };
                let saved_user_id = self.account.read(cx).deezer_user_id();
                let Ok(client) = self.client.clone() else {
                    self.finish_favorite(
                        &mutation,
                        Err("Library client could not be created".into()),
                        cx,
                    );
                    return;
                };
                self.runtime.spawn(async move {
                    client
                        .set_favorite(request.key, request.favorite, arl, saved_user_id)
                        .await
                })
            }
            crate::search::Provider::SoundCloud => {
                let (token, session_cookies) = {
                    let account = self.account.read(cx);
                    (
                        account.soundcloud_mobile_token(),
                        account.soundcloud_session_cookies(),
                    )
                };
                let Some(token) = token else {
                    self.finish_favorite(&mutation, Err("SoundCloud account required".into()), cx);
                    return;
                };
                let Some(session_cookies) = session_cookies else {
                    self.finish_favorite(&mutation, Err("SoundCloud account required".into()), cx);
                    return;
                };
                let Ok(client) = self.soundcloud_library_client() else {
                    self.finish_favorite(
                        &mutation,
                        Err("Library client could not be created".into()),
                        cx,
                    );
                    return;
                };
                self.runtime.spawn(async move {
                    client
                        .set_favorite(request.key, request.favorite, token, session_cookies)
                        .await
                })
            }
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Favorite request failed".into()));
            this.update(cx, |this, cx| this.finish_favorite(&mutation, result, cx))
                .ok();
        })
        .detach();
    }

    fn finish_favorite(
        &mut self,
        mutation: &super::favorite_state::FavoriteMutation,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let succeeded = result.is_ok();
        if let Err(ref error) = result {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Favorite failed",
                Some(error.clone().into()),
            );
        }
        let completed = self
            .favorites
            .update(cx, |favorites, _| favorites.complete(mutation, result));
        if !completed {
            return;
        }
        if succeeded {
            self.refresh_after_favorite(mutation.key.provider, category_for(mutation.key.kind), cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn refresh_after_favorite(
        &mut self,
        provider: crate::search::Provider,
        category: Category,
        cx: &mut Context<Self>,
    ) {
        self.state.invalidate_provider(provider, category);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_favorite_kind_invalidates_its_matching_root_category() {
        assert_eq!(category_for(FavoriteKind::Track), Category::Tracks);
        assert_eq!(category_for(FavoriteKind::Album), Category::Albums);
        assert_eq!(category_for(FavoriteKind::Artist), Category::Artists);
        assert_eq!(category_for(FavoriteKind::Playlist), Category::Playlists);
    }

    #[gpui::test]
    fn favorite_toggle_without_arl_pushes_error_toast(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;

        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let toasts = cx.update(|cx| cx.new(|_| crate::toast::ToastStack::new()));
        cx.update(|cx| {
            crate::toast::set_global(cx, &toasts);
        });
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let account = cx.update(|cx| {
            cx.new(|_| {
                AccountState::new(
                    crate::murglar_backend::DeviceIdentityStatus::Error(
                        crate::murglar_backend::DeviceIdentityError::Serialization,
                    ),
                    Err(crate::account_session::SessionError::Filesystem),
                )
            })
        });
        let favorites =
            cx.update(|cx| cx.new(|_| super::super::favorite_state::FavoriteState::default()));
        let controller = cx
            .update(|cx| cx.new(|_| FavoriteController::new(account, favorites.clone(), runtime)));

        cx.update(|cx| {
            controller.update(cx, |controller, cx| {
                controller.toggle(
                    FavoriteKey::deezer(FavoriteKind::Track, "42".into()),
                    false,
                    cx,
                );
            });
        });

        cx.update(|cx| {
            assert_eq!(
                favorites
                    .read(cx)
                    .favorite(&FavoriteKey::deezer(FavoriteKind::Track, "42".into())),
                None
            );
        });
    }
}
