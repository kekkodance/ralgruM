//! Provider-neutral lyrics domain, parsing, and response-shape extraction.
//!
//! Network requests, UI rendering, and current-generation validation stay in
//! the surrounding lyrics application module.

mod cache;
mod genius_match;
mod models;
mod parsers;
mod providers;

pub use cache::{LyricsCache, LyricsCacheStore};
pub use genius_match::{
    GeniusHit, finalize_genius_hit, genius_search_queries, record_genius_hit, select_genius_hit,
};
pub use models::{
    EmptyLyricsReason, LyricAnnotation, LyricLine, LyricsCacheKey, LyricsProvider, LyricsResponse,
    LyricsTrack,
};
pub use parsers::{
    GeniusLyrics, LyricFragment, collapse_blank_lyric_gaps, genius_line_fragments,
    lyric_block_text, lyric_full_text, parse_synced_lyrics, prepare_genius_lyrics,
};
pub use providers::{
    extract_genius_lyrics, extract_genius_lyrics_result, extract_genius_referents,
    extract_musixmatch_lyrics,
};

#[cfg(test)]
pub use providers::extract_genius_hits;
