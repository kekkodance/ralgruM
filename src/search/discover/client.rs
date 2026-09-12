use super::model::DiscoverSection;
use crate::search::SearchClient;
use crate::search::credential::{DeezerArl, SoundCloudToken};
use crate::search::models::Provider;

pub(crate) async fn load(
    provider: Provider,
    client: &SearchClient,
    deezer_arl: Option<DeezerArl>,
    soundcloud_token: Option<SoundCloudToken>,
) -> Result<Vec<DiscoverSection>, String> {
    match provider {
        Provider::Deezer => {
            let arl = deezer_arl.ok_or_else(|| "Deezer account required".to_owned())?;
            super::deezer::load(client, arl).await
        }
        Provider::SoundCloud => super::soundcloud::load(client, soundcloud_token).await,
    }
}
