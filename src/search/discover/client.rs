use super::model::DiscoverSection;
use crate::navigation_state::Language;
use crate::search::SearchClient;
use crate::search::credential::{DeezerArl, SoundCloudToken};
use crate::search::models::Provider;

pub(crate) async fn load(
    provider: Provider,
    client: &SearchClient,
    deezer_arl: Option<DeezerArl>,
    soundcloud_token: Option<SoundCloudToken>,
    language: Language,
) -> Result<Vec<DiscoverSection>, String> {
    match provider {
        Provider::Deezer => {
            let arl = deezer_arl.ok_or_else(|| "Deezer account required".to_owned())?;
            super::deezer::load(client, arl, language).await
        }
        Provider::SoundCloud => super::soundcloud::load(client, soundcloud_token, language).await,
    }
}
