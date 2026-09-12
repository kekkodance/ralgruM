use super::{
    client::{DeezerSession, LibraryClient, root_page},
    deezer_radio::{result_items, result_total},
    model::{Category, Page},
};
use serde_json::{Value, json};

const HISTORY_PAGE_SIZE: usize = 200;
const MAX_HISTORY_PAGES: usize = 50;

impl LibraryClient {
    pub(super) async fn load_history(
        &self,
        session: DeezerSession,
        user_id: &str,
    ) -> Result<Page, String> {
        let mut tracks = Vec::new();
        let mut raw_loaded_count = 0usize;
        let mut reported_total = 0usize;

        for page_index in 0..MAX_HISTORY_PAGES {
            let start = page_index * HISTORY_PAGE_SIZE;
            let results = self
                .gateway_call(
                    "user.getSongsHistory",
                    history_request(user_id, start),
                    &session.token,
                    session.cookie.clone(),
                )
                .await?;
            let items = result_items(&results);
            if page_index == 0 {
                reported_total = result_total(&results, items.len());
            }
            if history_page_is_terminal(&items) {
                break;
            }
            raw_loaded_count += items.len();
            tracks.extend(self.hydrate_deezer_tracks(items, &session).await?);
        }

        let total = if reported_total == 0 {
            tracks.len()
        } else {
            reported_total
        };
        let mut page = root_page(Category::History, total);
        page.raw_loaded_count = raw_loaded_count;
        page.normalized_count = tracks.len();
        page.tracks = tracks;
        Ok(page)
    }
}

fn history_request(user_id: &str, start: usize) -> Value {
    json!({ "user_id": user_id, "nb": HISTORY_PAGE_SIZE, "start": start })
}

fn history_page_is_terminal(items: &[Value]) -> bool {
    items.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_requests_match_captured_two_hundred_item_offsets() {
        assert_eq!(
            history_request("42", 0),
            json!({ "user_id": "42", "nb": 200, "start": 0 })
        );
        assert_eq!(
            history_request("42", HISTORY_PAGE_SIZE),
            json!({ "user_id": "42", "nb": 200, "start": 200 })
        );
        assert_eq!(
            history_request("42", HISTORY_PAGE_SIZE * 2),
            json!({ "user_id": "42", "nb": 200, "start": 400 })
        );
    }

    #[test]
    fn short_nonempty_history_pages_do_not_stop_pagination() {
        let captured_first_page = vec![json!({ "SNG_ID": "100" }); 100];
        assert!(!history_page_is_terminal(&captured_first_page));
        assert!(history_page_is_terminal(&[]));
    }

    #[test]
    fn history_paging_is_bounded() {
        assert_eq!(HISTORY_PAGE_SIZE, 200);
        assert_eq!(MAX_HISTORY_PAGES * HISTORY_PAGE_SIZE, 10_000);
    }
}
