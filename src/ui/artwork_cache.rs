use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use futures::FutureExt;
use gpui::{
    App, AppContext, Asset, AssetLogger, Entity, Image, ImageAssetLoader, ImageCache,
    ImageCacheError, ImageCacheItem, ImageFormat, ImageLoadingTask, RenderImage, Resource,
    SvgRenderer, Window,
};
use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::runtime::Runtime;

const ARTWORK_CACHE_DIR: &str = "artwork-v1";
const ARTWORK_CACHE_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
const ARTWORK_MAX_BYTES: u64 = 16 * 1024 * 1024;
// Search can keep several 64-card carousels mounted at once. The cache must
// retain every mounted cover so one section cannot evict another mid-frame.
const ARTWORK_MEMORY_CACHE_LIMIT: usize = 512;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Persistent artwork storage backed by GPUI's in-memory image cache.
///
/// URI resources are downloaded once, validated, and atomically stored on disk.
/// GPUI still performs the final image decode, so disk and network resources use
/// the same image handling and error behavior.
pub(crate) struct ArtworkCache {
    cache_dir: PathBuf,
    runtime: Arc<Runtime>,
    client: Client,
    items: HashMap<Resource, ImageCacheItem>,
    access_order: LruOrder<Resource>,
}

impl ArtworkCache {
    fn new(cache_dir: PathBuf, runtime: Arc<Runtime>, client: Client) -> Self {
        let _ = prune_cache_dir(&cache_dir, ARTWORK_CACHE_LIMIT_BYTES);
        Self {
            cache_dir,
            runtime,
            client,
            items: HashMap::new(),
            access_order: LruOrder::default(),
        }
    }

    /// Construct the application artwork cache and register image cleanup.
    pub(crate) fn new_entity(runtime: Arc<Runtime>, cx: &mut App) -> Entity<Self> {
        let cache_dir = crate::paths::cache_dir().join(ARTWORK_CACHE_DIR);
        let client = Client::builder()
            .user_agent("ralgrum-gpui-artwork")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to create artwork HTTP client");
        let cache = cx.new(|_| Self::new(cache_dir, runtime, client));
        cx.observe_release(&cache, |cache, cx| cache.release(cx))
            .detach();
        cache
    }

    fn release(&mut self, cx: &mut App) {
        for (_, mut item) in self.items.drain() {
            if let Some(Ok(image)) = item.get() {
                cx.drop_image(image, None);
            }
        }
        self.access_order.clear();
    }

    fn load_regular_resource(
        &mut self,
        source: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let loader = AssetLogger::<ImageAssetLoader>::load(source.clone(), cx);
        let task = cx.background_executor().spawn(loader).shared();
        self.insert_loading(source.clone(), task, window, cx)
    }

    fn insert_loading(
        &mut self,
        source: Resource,
        task: ImageLoadingTask,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.items
            .insert(source.clone(), ImageCacheItem::Loading(task.clone()));
        self.access_order.touch(&source);
        self.trim_memory_cache(Some(&source), window, cx);
        let entity = window.current_view();
        window
            .spawn(cx, async move |cx| {
                let _ = task.await;
                cx.on_next_frame(move |_, cx| cx.notify(entity));
            })
            .detach();
        None
    }

    fn trim_memory_cache(
        &mut self,
        protected: Option<&Resource>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Loading tasks own the only in-flight request for their URI. Retain
        // them even while trimming so a cache limit cannot turn one image into
        // several concurrent downloads.
        let mut remaining_candidates = self.access_order.len();
        while self.items.len() > ARTWORK_MEMORY_CACHE_LIMIT && remaining_candidates > 0 {
            let Some(victim) = self.access_order.pop_oldest_except(protected) else {
                break;
            };
            let is_loading = self
                .items
                .get_mut(&victim)
                .is_some_and(|item| item.get().is_none());
            if is_loading {
                self.access_order.touch(&victim);
                remaining_candidates -= 1;
                continue;
            }
            if let Some(mut item) = self.items.remove(&victim) {
                if let Some(Ok(image)) = item.get() {
                    cx.drop_image(image, Some(window));
                }
            }
            remaining_candidates = self.access_order.len();
        }
    }

    fn load_uri(
        &mut self,
        source: &Resource,
        uri: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let cache_path = cache_path(&self.cache_dir, uri);
        let path_resource = Resource::Path(cache_path.clone().into());
        let path_loader = AssetLogger::<ImageAssetLoader>::load(path_resource, cx);
        let runtime = self.runtime.clone();
        let client = self.client.clone();
        let cache_dir = self.cache_dir.clone();
        let url = uri.to_owned();
        let probe_path = cache_path.clone();
        let decode_path = cache_path.clone();
        let svg_renderer = cx.svg_renderer();
        let future = async move {
            let cached = runtime
                .spawn_blocking(move || disk_cache_candidate(&probe_path))
                .await
                .ok()
                .flatten();
            if cached.is_some() {
                match path_loader.await {
                    Ok(image) => return Ok(image),
                    Err(_) => {
                        let stale_path = decode_path.clone();
                        let _ = runtime
                            .spawn_blocking(move || remove_if_unchanged(&stale_path, cached))
                            .await;
                    }
                }
            }

            let bytes = download_artwork(client, url.clone())
                .await
                .map_err(image_error)?;
            let image = decode_artwork(runtime.clone(), svg_renderer, bytes.clone()).await?;

            // Persistence is deliberately detached from the render path. The
            // validated render image is ready now; disk writes and pruning must
            // not delay the first paint.
            let persist_runtime = runtime.clone();
            let persistence = runtime.spawn(async move {
                let _ = persist_artwork(persist_runtime, cache_dir, decode_path, bytes).await;
            });
            drop(persistence);
            Ok(image)
        }
        .boxed();
        let task = cx.background_executor().spawn(future).shared();
        self.insert_loading(source.clone(), task, window, cx)
    }
}

impl ImageCache for ArtworkCache {
    fn load(
        &mut self,
        source: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        if let Some(item) = self.items.get_mut(source) {
            let result = item.get();
            self.access_order.touch(source);
            self.trim_memory_cache(Some(source), window, cx);
            return result;
        }

        match source {
            Resource::Uri(uri) => self.load_uri(source, uri.as_ref(), window, cx),
            Resource::Path(_) | Resource::Embedded(_) => {
                self.load_regular_resource(source, window, cx)
            }
        }
    }
}

#[derive(Debug)]
struct LruOrder<K> {
    entries: VecDeque<K>,
}

impl<K> Default for LruOrder<K> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}

impl<K: Clone + Eq> LruOrder<K> {
    fn touch(&mut self, key: &K) {
        if let Some(position) = self.entries.iter().position(|entry| entry == key) {
            self.entries.remove(position);
        }
        self.entries.push_back(key.clone());
    }

    fn pop_oldest_except(&mut self, protected: Option<&K>) -> Option<K> {
        let position = self
            .entries
            .iter()
            .position(|entry| protected.is_none_or(|protected| entry != protected))?;
        self.entries.remove(position)
    }

    fn clear(&mut self) {
        self.entries.clear();
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

fn cache_path(cache_dir: &Path, url: &str) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(url.as_bytes());
    let digest = digest.finalize();
    let filename = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    cache_dir.join(format!("{filename}.image"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CacheFingerprint {
    size: u64,
    modified: SystemTime,
}

fn cache_fingerprint(path: &Path) -> Option<CacheFingerprint> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(CacheFingerprint {
        size: metadata.len(),
        modified: metadata.modified().ok()?,
    })
}

fn disk_cache_candidate(path: &Path) -> Option<CacheFingerprint> {
    match cache_fingerprint(path) {
        Some(fingerprint) if fingerprint.size <= ARTWORK_MAX_BYTES => Some(fingerprint),
        Some(fingerprint) => {
            remove_if_unchanged(path, Some(fingerprint));
            None
        }
        None => None,
    }
}

fn remove_if_unchanged(path: &Path, expected: Option<CacheFingerprint>) {
    if expected.is_some() && cache_fingerprint(path) == expected {
        let _ = fs::remove_file(path);
    }
}

async fn download_artwork(client: Client, url: String) -> Result<Vec<u8>, String> {
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("artwork request returned {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > ARTWORK_MAX_BYTES)
    {
        return Err("artwork response is too large".to_owned());
    }

    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len() as u64 + chunk.len() as u64 > ARTWORK_MAX_BYTES {
            return Err("artwork response is too large".to_owned());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn decode_artwork(
    runtime: Arc<Runtime>,
    svg_renderer: SvgRenderer,
    bytes: Vec<u8>,
) -> Result<Arc<RenderImage>, ImageCacheError> {
    runtime
        .spawn_blocking(move || {
            let format = supported_image_format(&bytes)
                .ok_or_else(|| image_error("artwork response is not a supported image"))?;
            Image::from_bytes(format, bytes)
                .to_image_data(svg_renderer)
                .map_err(|error| ImageCacheError::Other(Arc::new(error)))
        })
        .await
        .map_err(|_| image_error("artwork image decoder stopped unexpectedly"))?
}

async fn persist_artwork(
    runtime: Arc<Runtime>,
    cache_dir: PathBuf,
    cache_path: PathBuf,
    bytes: Vec<u8>,
) -> Result<(), String> {
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|error| error.to_string())?;
    write_atomically(&cache_path, &bytes).await?;
    let prune_dir = cache_dir.clone();
    runtime
        .spawn_blocking(move || prune_cache_dir(&prune_dir, ARTWORK_CACHE_LIMIT_BYTES))
        .await
        .map_err(|_| "artwork cache pruner stopped unexpectedly".to_owned())??;
    Ok(())
}

fn supported_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Jpeg => Some(ImageFormat::Jpeg),
        image::ImageFormat::Png => Some(ImageFormat::Png),
        image::ImageFormat::WebP => Some(ImageFormat::Webp),
        _ => None,
    }
}

fn image_error(message: impl Into<String>) -> ImageCacheError {
    std::io::Error::other(message.into()).into()
}

async fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary_path = temporary_path(path);
    if let Err(error) = tokio::fs::write(&temporary_path, bytes).await {
        let _ = tokio::fs::remove_file(&temporary_path).await;
        return Err(error.to_string());
    }

    match tokio::fs::rename(&temporary_path, path).await {
        Ok(()) => Ok(()),
        Err(error) if path.exists() => {
            let _ = tokio::fs::remove_file(&temporary_path).await;
            let _ = error;
            Ok(())
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary_path).await;
            Err(error.to_string())
        }
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = path
        .file_name()
        .and_then(|filename| filename.to_str())
        .unwrap_or("artwork.image");
    path.with_file_name(format!(".{filename}.{}-{counter}.tmp", std::process::id()))
}

#[derive(Debug, Clone)]
struct CacheFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

fn is_cache_file(path: &Path) -> bool {
    !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
}

fn files_to_prune(mut files: Vec<CacheFile>, limit: u64) -> Vec<PathBuf> {
    let mut total = files
        .iter()
        .map(|file| file.size)
        .fold(0_u64, u64::saturating_add);
    files.sort_by(|left, right| {
        left.modified
            .cmp(&right.modified)
            .then_with(|| left.path.cmp(&right.path))
    });

    let mut removed = Vec::new();
    for file in files {
        if total <= limit {
            break;
        }
        total = total.saturating_sub(file.size);
        removed.push(file.path);
    }
    removed
}

fn prune_cache_dir(cache_dir: &Path, limit: u64) -> Result<(), String> {
    let entries = fs::read_dir(cache_dir).map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || !is_cache_file(&path) {
            continue;
        }
        files.push(CacheFile {
            path,
            size: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }

    for path in files_to_prune(files, limit) {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_uses_sha256_url_key() {
        let path = cache_path(Path::new("cache"), "https://example.test/artwork.jpg");
        assert_eq!(
            path,
            PathBuf::from(
                "cache/92f1985235143b3546238c25ca066d7a0382f83566a30848b17c11242bf99086.image"
            )
        );
    }

    #[test]
    fn pruning_removes_oldest_files_until_under_limit() {
        let files = vec![
            CacheFile {
                path: PathBuf::from("old"),
                size: 4,
                modified: SystemTime::UNIX_EPOCH,
            },
            CacheFile {
                path: PathBuf::from("middle"),
                size: 6,
                modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
            },
            CacheFile {
                path: PathBuf::from("new"),
                size: 5,
                modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2),
            },
        ];
        assert_eq!(
            files_to_prune(files, 10),
            [PathBuf::from("old"), PathBuf::from("middle")]
        );
    }

    #[test]
    fn temporary_suffix_is_not_a_cache_file() {
        assert!(!is_cache_file(Path::new("cache/.artwork.image.1.tmp")));
        assert!(is_cache_file(Path::new("cache/artwork.image")));
    }

    #[test]
    fn temporary_paths_are_process_unique() {
        let path = Path::new("cache/artwork.image");
        let first = temporary_path(path);
        let second = temporary_path(path);
        assert_ne!(first, second);
        assert!(
            first
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(&std::process::id().to_string()))
        );
    }

    #[test]
    fn lru_touch_refreshes_recency_without_duplicates() {
        let mut order = LruOrder::default();
        order.touch(&"first");
        order.touch(&"second");
        order.touch(&"first");

        assert_eq!(order.pop_oldest_except(None), Some("second"));
        assert_eq!(order.pop_oldest_except(None), Some("first"));
        assert_eq!(order.pop_oldest_except(None), None);
    }

    #[test]
    fn lru_protects_requested_entry_when_selecting_victim() {
        let mut order = LruOrder::default();
        order.touch(&"oldest");
        order.touch(&"middle");
        order.touch(&"newest");

        assert_eq!(order.pop_oldest_except(Some(&"oldest")), Some("middle"));
        assert_eq!(order.pop_oldest_except(Some(&"oldest")), Some("newest"));
        assert_eq!(order.pop_oldest_except(Some(&"oldest")), None);
    }

    #[test]
    fn lru_trim_keeps_entries_at_the_configured_cap() {
        let mut order = LruOrder::default();
        for key in 0..5 {
            order.touch(&key);
        }

        let cap = 2;
        let protected = 4;
        let mut evicted = Vec::new();
        while order.entries.len() > cap {
            evicted.push(order.pop_oldest_except(Some(&protected)).unwrap());
        }

        assert_eq!(evicted, [0, 1, 2]);
        assert_eq!(order.entries.into_iter().collect::<Vec<_>>(), [3, 4]);
    }

    #[test]
    fn memory_limit_keeps_all_search_preview_artwork_resident() {
        const SEARCH_PREVIEW_SECTIONS: usize = 4;
        const DOUBLE_BUFFERED_FRAMES: usize = 2;
        let required = crate::music_ui::CAROUSEL_STABLE_CARD_LIMIT
            * SEARCH_PREVIEW_SECTIONS
            * DOUBLE_BUFFERED_FRAMES;

        assert!(ARTWORK_MEMORY_CACHE_LIMIT >= required);
    }

    #[test]
    fn stale_fingerprint_does_not_remove_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artwork.image");
        fs::write(&path, b"old").unwrap();
        let fingerprint = cache_fingerprint(&path);
        fs::write(&path, b"new replacement").unwrap();
        remove_if_unchanged(&path, fingerprint);
        assert!(path.exists());
    }

    #[test]
    fn supported_image_format_accepts_only_configured_rasters() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(
            supported_image_format(png.get_ref()),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            supported_image_format(b"<svg xmlns=\"http://www.w3.org/2000/svg\">"),
            None
        );
        assert_eq!(supported_image_format(b"not an image"), None);
    }

    #[test]
    fn incomplete_raster_is_deferred_to_the_final_decoder() {
        // Format detection rejects unsupported data up front, while the final
        // GPUI decoder validates complete raster bytes exactly once.
        assert_eq!(
            supported_image_format(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            supported_image_format(b"<svg xmlns=\"http://www.w3.org/2000/svg\">"),
            None
        );
    }
}
