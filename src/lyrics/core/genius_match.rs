use serde_json::Value;

const STRONG_SCORE: i32 = 90;
const MIN_SCORE: i32 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeniusHit {
    pub id: u64,
    pub url: String,
    pub score: i32,
    artist_overlap: bool,
}

pub fn genius_search_queries(artist: &str, title: &str) -> Vec<String> {
    let title = strip_credit_suffix(title);
    let artist = artist.trim();
    let mut queries = Vec::new();
    push_unique(&mut queries, &title);
    if !artist.is_empty() && !title.is_empty() {
        push_unique(&mut queries, &format!("{title} {artist}"));
        push_unique(&mut queries, &format!("{artist} {title}"));
    }
    queries
}

pub fn is_strong_genius_hit(hit: &GeniusHit, artist: &str) -> bool {
    if hit.score < STRONG_SCORE {
        return false;
    }
    artist.trim().is_empty() || hit.artist_overlap
}

pub fn select_genius_hit(search: &Value, artist: &str, title: &str) -> Option<GeniusHit> {
    let title = strip_credit_suffix(title);
    let artist = artist.trim();
    let query_title = normalize(&title);
    let query_wants_version = has_version_tag(&title);
    let mut best: Option<GeniusHit> = None;
    for (index, result) in genius_hit_results(search).into_iter().enumerate() {
        if is_translation_hit(result) {
            continue;
        }
        let Some(id) = result.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let Some(url) = result
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
        else {
            continue;
        };
        let hit_title = result
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let score = score_hit(
            &query_title,
            artist,
            result,
            hit_title,
            query_wants_version,
            index,
        );
        if score < MIN_SCORE {
            continue;
        }
        let candidate = GeniusHit {
            id,
            url: url.to_owned(),
            score,
            artist_overlap: artist_overlap(artist, result),
        };
        if best
            .as_ref()
            .is_none_or(|current| prefers_genius_hit(&candidate, current))
        {
            best = Some(candidate);
        }
    }
    best
}

pub fn record_genius_hit(
    best_overlap: &mut Option<GeniusHit>,
    best_any: &mut Option<GeniusHit>,
    candidate: GeniusHit,
    artist: &str,
) -> Option<GeniusHit> {
    // Strong overlap stops the query loop. Any overlap later beats a higher-scoring miss.
    if is_strong_genius_hit(&candidate, artist) {
        return Some(candidate);
    }
    if candidate.artist_overlap {
        replace_higher_score(best_overlap, candidate.clone());
    }
    replace_higher_score(best_any, candidate);
    None
}

pub fn finalize_genius_hit(
    best_overlap: Option<GeniusHit>,
    best_any: Option<GeniusHit>,
    artist: &str,
    title: &str,
) -> Option<GeniusHit> {
    if best_overlap.is_some() {
        return best_overlap;
    }
    let candidate = best_any?;
    if is_missing_artist(artist) {
        return is_strong_genius_hit(&candidate, "").then_some(candidate);
    }
    let normalized_title = normalize(&strip_credit_suffix(title));
    let distinctive_title = normalized_title.split_whitespace().count() >= 4;
    (distinctive_title && candidate.score >= STRONG_SCORE).then_some(candidate)
}

fn is_missing_artist(artist: &str) -> bool {
    matches!(
        normalize(artist).as_str(),
        "" | "unknown artist" | "various artists"
    )
}

fn prefers_genius_hit(candidate: &GeniusHit, current: &GeniusHit) -> bool {
    match (candidate.artist_overlap, current.artist_overlap) {
        (true, false) => true,
        (false, true) => false,
        _ => candidate.score > current.score,
    }
}

fn replace_higher_score(slot: &mut Option<GeniusHit>, candidate: GeniusHit) {
    if slot
        .as_ref()
        .is_none_or(|current| candidate.score > current.score)
    {
        *slot = Some(candidate);
    }
}

fn genius_hit_results(search: &Value) -> Vec<&Value> {
    let mut results = Vec::new();
    if let Some(sections) = search
        .pointer("/response/sections")
        .and_then(Value::as_array)
    {
        for section in sections {
            let hits = section.get("hits").and_then(Value::as_array);
            push_hit_results(&mut results, hits);
        }
    }
    if results.is_empty() {
        push_hit_results(
            &mut results,
            search.pointer("/response/hits").and_then(Value::as_array),
        );
    }
    results
}

fn push_hit_results<'a>(results: &mut Vec<&'a Value>, hits: Option<&'a Vec<Value>>) {
    let Some(hits) = hits else {
        return;
    };
    for hit in hits {
        if let Some(result) = hit.get("result") {
            results.push(result);
        }
    }
}

fn score_hit(
    query_title: &str,
    artist: &str,
    result: &Value,
    hit_title: &str,
    query_wants_version: bool,
    index: usize,
) -> i32 {
    if query_title.is_empty() {
        return 0;
    }
    let hit_core = normalize(&strip_credit_suffix(hit_title));
    let mut score = if hit_core == query_title {
        100
    } else if query_title.len() >= 8 && hit_core.contains(query_title) {
        45
    } else if hit_core.len() >= 8 && query_title.contains(&hit_core) {
        40
    } else {
        0
    };
    if !query_wants_version && has_version_tag(hit_title) {
        score -= 80;
    }
    if artist_overlap(artist, result) {
        score += 35;
    }
    score - index as i32
}

fn artist_overlap(artist: &str, result: &Value) -> bool {
    let query = tokenize(artist);
    if query.is_empty() {
        return false;
    }
    let mut haystack = String::new();
    for pointer in [
        "/primary_artist/name",
        "/artist_names",
        "/primary_artist_names",
    ] {
        if let Some(value) = result.pointer(pointer).and_then(Value::as_str) {
            haystack.push(' ');
            haystack.push_str(value);
        }
    }
    if let Some(artists) = result.get("featured_artists").and_then(Value::as_array) {
        for artist in artists {
            if let Some(name) = artist.get("name").and_then(Value::as_str) {
                haystack.push(' ');
                haystack.push_str(name);
            }
        }
    }
    let haystack = tokenize(&haystack);
    query.iter().any(|token| haystack.contains(token))
}

fn is_translation_hit(result: &Value) -> bool {
    let title = result.get("title").and_then(Value::as_str).unwrap_or("");
    let url = result.get("url").and_then(Value::as_str).unwrap_or("");
    let path = result.get("path").and_then(Value::as_str).unwrap_or("");
    let artist = result
        .pointer("/primary_artist/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let blob = format!("{title} {url} {path} {artist}").to_lowercase();
    [
        "traduccion",
        "traducción",
        "traducao",
        "tradução",
        "traducoes",
        "traduções",
        "traducciones",
        "traduzione",
        "traduzioni",
        "traduction",
        "translation",
        "translations",
        "çeviri",
        "ceviri",
        "tłumaczenie",
        "tumaczenie",
        "tłumaczenia",
        "tumaczenia",
        "übersetzung",
        "ubersetzung",
        "перевод",
        "romanization",
        "romanized",
        "genius-traducciones",
        "genius-brasil-traducoes",
        "genius-traduzioni",
        "genius-turkce",
        "genius-greek-translations",
        "polskie-tumaczenia",
    ]
    .iter()
    .any(|marker| blob.contains(marker))
}

fn has_version_tag(title: &str) -> bool {
    let normalized = format!(" {} ", normalize(title));
    [
        " remix ",
        " live ",
        " mashup ",
        " acoustic ",
        " cover ",
        " flip ",
        " bootleg ",
        " karaoke ",
        " instrumental ",
        " reprise ",
        " edit ",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

pub fn strip_credit_suffix(title: &str) -> String {
    let mut current = title.trim().to_owned();
    loop {
        let Some((start, end)) = find_credit_group(&current) else {
            return current;
        };
        current = remove_group(&current, start, end);
    }
}

fn find_credit_group(title: &str) -> Option<(usize, usize)> {
    let mut groups = Vec::new();
    for (index, character) in title.char_indices() {
        match character {
            '(' | '[' => groups.push((character, index)),
            ')' | ']' => {
                let Some((open, start)) = groups.pop() else {
                    continue;
                };
                let matches = matches!((open, character), ('(', ')') | ('[', ']'));
                if !matches {
                    groups.clear();
                    continue;
                }
                let inner_start = start + open.len_utf8();
                let inner = &title[inner_start..index];
                if is_credit_group(inner) {
                    return Some((start, index + character.len_utf8()));
                }
            }
            _ => {}
        }
    }
    None
}

fn is_credit_group(inner: &str) -> bool {
    matches!(
        normalize(inner).split_whitespace().next(),
        Some("feat" | "ft" | "featuring" | "with")
    )
}

fn remove_group(title: &str, start: usize, end: usize) -> String {
    let before = title[..start].trim_end();
    let after = title[end..].trim_start();
    match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (false, true) => before.to_owned(),
        (true, false) => after.to_owned(),
        (false, false) => format!("{before} {after}"),
    }
}

fn tokenize(value: &str) -> Vec<String> {
    normalize(value)
        .split_whitespace()
        .filter(|token| is_significant_artist_token(token))
        .map(str::to_owned)
        .collect()
}

fn is_significant_artist_token(token: &str) -> bool {
    token.len() >= 3
        && ![
            "the", "and", "of", "a", "an", "to", "for", "by", "vs", "feat", "ft", "with", "from",
        ]
        .contains(&token)
}

fn normalize(value: &str) -> String {
    let mut folded = String::new();
    for character in value.chars() {
        match character {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' => {
                folded.push('a')
            }
            'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => folded.push('e'),
            'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => folded.push('i'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => folded.push('o'),
            'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => folded.push('u'),
            'ñ' | 'Ñ' => folded.push('n'),
            'ç' | 'Ç' => folded.push('c'),
            character if character.is_alphanumeric() => {
                folded.extend(character.to_lowercase());
            }
            _ => folded.push(' '),
        }
    }
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_unique(queries: &mut Vec<String>, query: &str) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    if queries.iter().any(|existing| existing == query) {
        return;
    }
    queries.push(query.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hit(id: u64, title: &str, artist: &str, url: &str) -> Value {
        json!({
            "result": {
                "id": id,
                "title": title,
                "url": url,
                "path": url.trim_start_matches("https://genius.com"),
                "primary_artist": { "name": artist }
            }
        })
    }

    fn search(hits: Vec<Value>) -> Value {
        json!({ "response": { "sections": [{ "type": "song", "hits": hits }] } })
    }

    fn pick_from_searches(artist: &str, title: &str, searches: &[Value]) -> Option<GeniusHit> {
        let mut best_overlap = None;
        let mut best_any = None;
        for search in searches {
            let Some(candidate) = select_genius_hit(search, artist, title) else {
                continue;
            };
            if let Some(hit) =
                record_genius_hit(&mut best_overlap, &mut best_any, candidate, artist)
            {
                return Some(hit);
            }
        }
        finalize_genius_hit(best_overlap, best_any, artist, title)
    }

    #[test]
    fn credit_suffixes_are_stripped_from_titles() {
        assert_eq!(
            strip_credit_suffix("Where Are Ü Now (with Justin Bieber)"),
            "Where Are Ü Now"
        );
        assert_eq!(strip_credit_suffix("Song [feat. Someone]"), "Song");
        assert_eq!(strip_credit_suffix("Song (Remix)"), "Song (Remix)");
    }

    #[test]
    fn credit_groups_are_removed_before_later_version_groups() {
        assert_eq!(
            strip_credit_suffix("Try It Out (with Alvin Risk) (Neon Mix)"),
            "Try It Out (Neon Mix)"
        );
        assert_eq!(
            strip_credit_suffix("Song [feat. Someone] (Live)"),
            "Song (Live)"
        );
    }

    #[test]
    fn credit_group_matching_does_not_remove_similar_words_or_versions() {
        for title in ["Song (Without You)", "Song (Within)", "Song (Neon Mix)"] {
            assert_eq!(strip_credit_suffix(title), title);
        }
    }

    #[test]
    fn try_it_out_selects_the_skrillex_and_alvin_risk_genius_page() {
        let search = search(vec![hit(
            384332,
            "Try It Out (Neon Mix)",
            "Skrillex & Alvin Risk",
            "https://genius.com/Skrillex-and-alvin-risk-try-it-out-neon-mix-lyrics",
        )]);
        let selected = select_genius_hit(
            &search,
            "Skrillex",
            "Try It Out (with Alvin Risk) (Neon Mix)",
        )
        .expect("the versioned Genius page should match");
        assert_eq!(selected.id, 384332);
    }

    #[test]
    fn jack_u_query_starts_with_the_cleaned_title() {
        let queries = genius_search_queries("Jack Ü", "Where Are Ü Now (with Justin Bieber)");
        assert_eq!(queries[0], "Where Are Ü Now");
        assert!(queries.iter().any(|query| query.contains("Jack Ü")));
        assert!(!queries.iter().any(|query| query.contains("with Justin")));
    }

    #[test]
    fn original_page_wins_over_translations_and_remixes() {
        let search = search(vec![
            hit(
                7778372,
                "Skrillex, Justin Bieber & Diplo - Where Are Ü Now (Traducción al Español)",
                "Genius Traducciones al Español",
                "https://genius.com/Genius-traducciones-al-espanol-skrillex-justin-bieber-and-diplo-where-are-u-now-traduccion-al-espanol-lyrics",
            ),
            hit(
                12950848,
                "Skrillex, Justin Bieber & Diplo - Where Are Ü Now (Traduzione Italiana)",
                "Genius Traduzioni Italiane",
                "https://genius.com/Genius-traduzioni-italiane-skrillex-justin-bieber-and-diplo-where-are-u-now-traduzione-italiana-lyrics",
            ),
            hit(
                3329729,
                "Where Are Ü Now (Marshmello Remix)",
                "Skrillex",
                "https://genius.com/Skrillex-justin-bieber-and-diplo-where-are-u-now-marshmello-remix-lyrics",
            ),
            hit(
                713548,
                "Where Are Ü Now",
                "Skrillex",
                "https://genius.com/Skrillex-justin-bieber-and-diplo-where-are-u-now-lyrics",
            ),
        ]);
        let selected = select_genius_hit(&search, "Jack Ü", "Where Are Ü Now (with Justin Bieber)")
            .expect("original page");
        assert_eq!(selected.id, 713548);
        assert!(selected.score >= STRONG_SCORE);
    }

    #[test]
    fn remix_only_result_is_not_preferred_when_the_title_is_the_original() {
        let search = search(vec![hit(
            3405406,
            "Where Are Ü Now (Marshmello Remix) [Skrillex Flip]",
            "Jack Ü",
            "https://genius.com/Jack-u-where-are-u-now-marshmello-remix-skrillex-flip-lyrics",
        )]);
        assert!(
            select_genius_hit(&search, "Jack Ü", "Where Are Ü Now (with Justin Bieber)").is_none()
        );
    }

    #[test]
    fn empty_search_is_not_a_match() {
        assert!(select_genius_hit(&search(Vec::new()), "Skrillex", "Where Are Ü Now").is_none());
    }

    #[test]
    fn common_title_does_not_stop_on_an_unrelated_artist() {
        let mckameys = hit(
            239012,
            "Right on Time",
            "The McKameys",
            "https://genius.com/The-mckameys-right-on-time-lyrics",
        );
        let skrillex = hit(
            713549,
            "Right on Time",
            "Skrillex, 12th Planet & Kill The Noise",
            "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics",
        );
        let title_only = search(vec![
            mckameys.clone(),
            hit(
                239013,
                "Right on Time",
                "John Michael Montgomery",
                "https://genius.com/John-michael-montgomery-right-on-time-lyrics",
            ),
        ]);
        let first =
            select_genius_hit(&title_only, "Skrillex", "Right on Time").expect("title-only hit");
        assert_eq!(
            first.url,
            "https://genius.com/The-mckameys-right-on-time-lyrics"
        );
        assert!(!is_strong_genius_hit(&first, "Skrillex"));

        let artist_query = search(vec![mckameys, skrillex]);
        let selected = pick_from_searches("Skrillex", "Right on Time", &[title_only, artist_query])
            .expect("skrillex page");
        assert_eq!(
            selected.url,
            "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics"
        );
        assert!(is_strong_genius_hit(&selected, "Skrillex"));
    }

    #[test]
    fn kill_the_noise_does_not_overlap_the_mckameys() {
        let artist = "Skrillex, 12th Planet & Kill The Noise";
        let mckameys = hit(
            239012,
            "Right on Time",
            "The McKameys",
            "https://genius.com/The-mckameys-right-on-time-lyrics",
        );
        let skrillex = hit(
            713549,
            "Right on Time",
            "Skrillex, 12th Planet & Kill The Noise",
            "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics",
        );
        let title_only = search(vec![mckameys.clone()]);
        let first =
            select_genius_hit(&title_only, artist, "Right on Time").expect("title-only hit");
        assert_eq!(
            first.url,
            "https://genius.com/The-mckameys-right-on-time-lyrics"
        );
        assert!(!first.artist_overlap);
        assert!(!is_strong_genius_hit(&first, artist));

        let artist_query = search(vec![mckameys, skrillex]);
        let selected = pick_from_searches(artist, "Right on Time", &[title_only, artist_query])
            .expect("skrillex page");
        assert_eq!(
            selected.url,
            "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics"
        );
        assert!(selected.artist_overlap);
        assert!(is_strong_genius_hit(&selected, artist));
    }

    #[test]
    fn overlap_hit_wins_across_queries_even_with_a_lower_score() {
        let mckameys = hit(
            239012,
            "Right on Time",
            "The McKameys",
            "https://genius.com/The-mckameys-right-on-time-lyrics",
        );
        let skrillex = hit(
            713549,
            "Right on Time (Bangarang)",
            "Skrillex, 12th Planet & Kill The Noise",
            "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics",
        );
        let title_only = search(vec![mckameys]);
        let first =
            select_genius_hit(&title_only, "Skrillex", "Right on Time").expect("title-only hit");
        assert_eq!(
            first.url,
            "https://genius.com/The-mckameys-right-on-time-lyrics"
        );
        assert_eq!(first.score, 100);
        assert!(!first.artist_overlap);

        let artist_query = search(vec![skrillex]);
        let overlap =
            select_genius_hit(&artist_query, "Skrillex", "Right on Time").expect("overlap hit");
        assert_eq!(
            overlap.url,
            "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics"
        );
        assert!(overlap.artist_overlap);
        assert!(overlap.score < first.score);
        assert!(overlap.score >= MIN_SCORE);

        let selected = pick_from_searches("Skrillex", "Right on Time", &[title_only, artist_query])
            .expect("skrillex page");
        assert_eq!(
            selected.url,
            "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics"
        );
    }

    #[test]
    fn mixed_search_prefers_overlap_over_a_higher_scoring_title_miss() {
        let search = search(vec![
            hit(
                239012,
                "Right on Time",
                "The McKameys",
                "https://genius.com/The-mckameys-right-on-time-lyrics",
            ),
            hit(
                713549,
                "Right on Time (Bangarang)",
                "Skrillex, 12th Planet & Kill The Noise",
                "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics",
            ),
        ]);
        let selected =
            select_genius_hit(&search, "Skrillex", "Right on Time").expect("overlap hit");
        assert_eq!(
            selected.url,
            "https://genius.com/Skrillex-12th-planet-and-kill-the-noise-right-on-time-lyrics"
        );
        assert!(selected.artist_overlap);
        assert!(selected.score < 100);
    }

    #[test]
    fn jack_u_keeps_the_skrillex_original_when_no_query_overlaps() {
        let title_only = search(vec![hit(
            713548,
            "Where Are Ü Now",
            "Skrillex",
            "https://genius.com/Skrillex-justin-bieber-and-diplo-where-are-u-now-lyrics",
        )]);
        let first = select_genius_hit(
            &title_only,
            "Jack Ü",
            "Where Are Ü Now (with Justin Bieber)",
        )
        .expect("title-only original");
        assert_eq!(
            first.url,
            "https://genius.com/Skrillex-justin-bieber-and-diplo-where-are-u-now-lyrics"
        );
        assert!(!first.artist_overlap);
        assert!(!is_strong_genius_hit(&first, "Jack Ü"));

        let selected = pick_from_searches(
            "Jack Ü",
            "Where Are Ü Now (with Justin Bieber)",
            &[title_only, search(Vec::new())],
        )
        .expect("skrillex original");
        assert_eq!(
            selected.url,
            "https://genius.com/Skrillex-justin-bieber-and-diplo-where-are-u-now-lyrics"
        );
    }

    #[test]
    fn short_unrelated_title_only_matches_are_rejected_for_a_meaningful_artist() {
        let white_swan = search(vec![hit(
            1,
            "Overheat",
            "White Swan",
            "https://genius.com/White-swan-overheat-lyrics",
        )]);
        let haunted = search(vec![hit(
            2,
            "Brute Force",
            "The Haunted",
            "https://genius.com/The-haunted-brute-force-lyrics",
        )]);

        assert!(pick_from_searches("TRVCY", "Overheat", &[white_swan]).is_none());
        assert!(pick_from_searches("TRVCY", "Brute Force", &[haunted]).is_none());
    }

    #[test]
    fn finalization_keeps_try_it_out_artist_overlap() {
        let search = search(vec![hit(
            384332,
            "Try It Out (Neon Mix)",
            "Skrillex & Alvin Risk",
            "https://genius.com/Skrillex-and-alvin-risk-try-it-out-neon-mix-lyrics",
        )]);
        let candidate = select_genius_hit(
            &search,
            "Skrillex",
            "Try It Out (with Alvin Risk) (Neon Mix)",
        )
        .expect("artist-overlap candidate");
        let selected = finalize_genius_hit(
            Some(candidate),
            None,
            "Skrillex",
            "Try It Out (with Alvin Risk) (Neon Mix)",
        )
        .expect("artist-overlap hit");
        assert_eq!(selected.id, 384332);
    }
}
