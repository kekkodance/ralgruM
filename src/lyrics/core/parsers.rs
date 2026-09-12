use super::models::group_lyric_annotations;
use super::{LyricAnnotation, LyricLine};
use serde_json::Value;

pub fn parse_synced_lyrics(input: &str) -> Vec<LyricLine> {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    if let Ok(value) = serde_json::from_str::<Value>(&normalized) {
        if let Some(items) = value.as_array() {
            return items
                .iter()
                .enumerate()
                .map(|(index, item)| LyricLine {
                    time: item
                        .pointer("/time/total")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                    text: item
                        .get("text")
                        .filter(|value| !value.is_null() && value.as_bool() != Some(false))
                        .map(|value| match value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default(),
                    index,
                })
                .collect();
        }
        return Vec::new();
    }

    normalized
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let start = line.find('[')?;
            let (timestamp, text) = line[start + 1..].split_once(']')?;
            let (minutes, seconds) = timestamp.split_once(':')?;
            if minutes.is_empty()
                || seconds.is_empty()
                || !minutes.chars().all(|c| c.is_ascii_digit())
                || seconds.matches('.').count() != 1
                || seconds.split_once('.').is_none_or(|(whole, fraction)| {
                    whole.is_empty()
                        || fraction.is_empty()
                        || !whole.chars().all(|c| c.is_ascii_digit())
                        || !fraction.chars().all(|c| c.is_ascii_digit())
                })
            {
                return None;
            }
            let time = minutes.parse::<f64>().ok()? * 60.0 + seconds.parse::<f64>().ok()?;
            Some(LyricLine {
                time,
                text: text.trim().to_owned(),
                index,
            })
        })
        .collect()
}

/// Musixmatch timed lyrics often repeat blank rows through an instrumental.
/// Keep one gap so the view can show a single visualizer instead of a stack
/// of empty lines.
pub fn collapse_blank_lyric_gaps(lines: Vec<LyricLine>) -> Vec<LyricLine> {
    let mut collapsed = Vec::with_capacity(lines.len());
    let mut in_gap = false;
    for line in lines {
        if line.text.trim().is_empty() {
            if in_gap {
                continue;
            }
            in_gap = true;
            collapsed.push(LyricLine {
                time: line.time,
                text: String::new(),
                index: collapsed.len(),
            });
        } else {
            in_gap = false;
            collapsed.push(LyricLine {
                time: line.time,
                text: line.text,
                index: collapsed.len(),
            });
        }
    }
    while collapsed.len() > 1
        && collapsed
            .last()
            .is_some_and(|line| line.text.trim().is_empty())
    {
        collapsed.pop();
    }
    collapsed
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeniusLyrics {
    pub lyrics: String,
    pub lyric_lines: Vec<String>,
    pub line_block_indexes: Vec<i32>,
    pub ranges: Vec<(usize, usize, String)>,
    pub blocks: Vec<String>,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricFragment {
    pub text: String,
    pub annotation_id: Option<String>,
}

pub fn lyric_block_text(lines: &[String], index: usize) -> String {
    if index >= lines.len() || lines[index].trim().is_empty() {
        return String::new();
    }
    let mut start = index;
    let mut end = index;
    while start > 0 && !lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    while end + 1 < lines.len() && !lines[end + 1].trim().is_empty() {
        end += 1;
    }
    lines[start..=end]
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

pub fn lyric_full_text(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                ""
            } else {
                line.trim()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

pub fn genius_line_fragments(prepared: &GeniusLyrics, line_index: usize) -> Vec<LyricFragment> {
    let Some(line) = prepared.lyric_lines.get(line_index) else {
        return Vec::new();
    };
    let line_start = prepared.lyric_lines[..line_index]
        .iter()
        .map(|value| value.len() + 1)
        .sum::<usize>();
    let line_end = line_start + line.len();
    let mut fragments = Vec::new();
    let mut cursor = line_start;
    for &(start, end, ref id) in &prepared.ranges {
        if end <= line_start || start >= line_end {
            continue;
        }
        let start = start.max(line_start);
        let end = end.min(line_end);
        if start > cursor {
            fragments.push(LyricFragment {
                text: prepared.lyrics[cursor..start].to_owned(),
                annotation_id: None,
            });
        }
        fragments.push(LyricFragment {
            text: prepared.lyrics[start..end].to_owned(),
            annotation_id: Some(id.clone()),
        });
        cursor = end;
    }
    if cursor < line_end {
        fragments.push(LyricFragment {
            text: prepared.lyrics[cursor..line_end].to_owned(),
            annotation_id: None,
        });
    }
    trim_line_fragment_boundaries(&mut fragments);
    fragments
}

fn trim_line_fragment_boundaries(fragments: &mut Vec<LyricFragment>) {
    while let Some(first) = fragments.first_mut() {
        let trimmed = first.text.trim_start().to_owned();
        if trimmed.is_empty() {
            fragments.remove(0);
        } else {
            first.text = trimmed;
            break;
        }
    }
    while let Some(last) = fragments.last_mut() {
        let trimmed = last.text.trim_end().to_owned();
        if trimmed.is_empty() {
            fragments.pop();
        } else {
            last.text = trimmed;
            break;
        }
    }
}

pub fn prepare_genius_lyrics(
    text: &str,
    raw_annotations: &[LyricAnnotation],
    url: &str,
) -> GeniusLyrics {
    let lyrics = text.replace("\r\n", "\n").replace('\r', "\n");
    let lyric_lines: Vec<_> = lyrics.split('\n').map(str::to_owned).collect();
    let mut blocks: Vec<Vec<String>> = Vec::new();
    let mut line_block_indexes = Vec::with_capacity(lyric_lines.len());
    let mut previous_empty = true;
    for line in &lyric_lines {
        if line.trim().is_empty() {
            line_block_indexes.push(-1);
            previous_empty = true;
        } else {
            if previous_empty {
                blocks.push(Vec::new());
            }
            let index = blocks.len() as i32 - 1;
            blocks[index as usize].push(line.trim_end().to_owned());
            line_block_indexes.push(index);
            previous_empty = false;
        }
    }
    let mut occupied = vec![false; lyrics.len()];
    let mut ranges = Vec::new();
    let mut annotations = group_lyric_annotations(
        raw_annotations
            .iter()
            .filter(|annotation| {
                (!annotation.id.trim().is_empty()
                    || annotation.ids.iter().any(|id| !id.trim().is_empty()))
                    && !annotation.fragment.trim().is_empty()
            })
            .cloned()
            .collect(),
    );
    annotations.sort_by_key(|a| std::cmp::Reverse(a.fragment.len()));
    let searchable = lyrics.to_ascii_lowercase();
    for annotation in annotations {
        let needle = annotation.fragment.to_ascii_lowercase();
        let mut from = 0;
        while let Some(relative) = searchable[from..].find(&needle) {
            let start = from + relative;
            let end = start + needle.len();
            if !occupied[start..end].iter().any(|used| *used) {
                occupied[start..end].fill(true);
                ranges.push((start, end, annotation.range_id()));
            }
            from = end.max(start + 1);
            if from >= searchable.len() {
                break;
            }
        }
    }
    ranges.sort_by_key(|range| range.0);
    GeniusLyrics {
        lyrics,
        lyric_lines,
        line_block_indexes,
        ranges,
        blocks: blocks
            .into_iter()
            .map(|b| b.join("\n").trim().to_owned())
            .collect(),
        url: url.trim().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_and_lrc_fallback_match_contract() {
        assert_eq!(
            parse_synced_lyrics(r#"[{"time":{"total":1.5},"text":"Hi"},{"text":null}]"#)[1].time,
            0.0
        );
        assert_eq!(
            parse_synced_lyrics("bad\r[a]\n[01:02.50]  line\r[bad:1.0]"),
            vec![LyricLine {
                time: 62.5,
                text: "line".into(),
                index: 2
            }]
        );
    }

    #[test]
    fn synced_json_preserves_order_and_stringifies_values() {
        let lines = parse_synced_lyrics(
            r#"[{"time":{"total":2},"text":42},{"time":{},"text":false},{"text":"last"}]"#,
        );
        assert_eq!(
            lines.iter().map(|line| line.index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec!["42", "", "last"]
        );
        assert_eq!(lines[1].time, 0.0);
    }

    #[test]
    fn consecutive_blank_synced_lines_collapse_to_one_gap() {
        let lines = vec![
            LyricLine {
                time: 0.0,
                text: "Intro".into(),
                index: 0,
            },
            LyricLine {
                time: 1.0,
                text: "  ".into(),
                index: 1,
            },
            LyricLine {
                time: 1.5,
                text: String::new(),
                index: 2,
            },
            LyricLine {
                time: 2.0,
                text: "\t".into(),
                index: 3,
            },
            LyricLine {
                time: 3.0,
                text: "Verse".into(),
                index: 4,
            },
            LyricLine {
                time: 4.0,
                text: String::new(),
                index: 5,
            },
        ];
        let collapsed = collapse_blank_lyric_gaps(lines);
        assert_eq!(
            collapsed,
            vec![
                LyricLine {
                    time: 0.0,
                    text: "Intro".into(),
                    index: 0,
                },
                LyricLine {
                    time: 1.0,
                    text: String::new(),
                    index: 1,
                },
                LyricLine {
                    time: 3.0,
                    text: "Verse".into(),
                    index: 2,
                },
            ]
        );
    }

    #[test]
    fn trailing_blank_gaps_are_dropped_but_a_blank_only_list_is_kept() {
        let trailing = collapse_blank_lyric_gaps(vec![
            LyricLine {
                time: 0.0,
                text: "Outro".into(),
                index: 0,
            },
            LyricLine {
                time: 1.0,
                text: String::new(),
                index: 1,
            },
            LyricLine {
                time: 2.0,
                text: "  ".into(),
                index: 2,
            },
        ]);
        assert_eq!(
            trailing,
            vec![LyricLine {
                time: 0.0,
                text: "Outro".into(),
                index: 0,
            }]
        );

        let blank_only = collapse_blank_lyric_gaps(vec![
            LyricLine {
                time: 0.0,
                text: String::new(),
                index: 0,
            },
            LyricLine {
                time: 1.0,
                text: String::new(),
                index: 1,
            },
        ]);
        assert_eq!(
            blank_only,
            vec![LyricLine {
                time: 0.0,
                text: String::new(),
                index: 0,
            }]
        );
    }

    #[test]
    fn valid_non_array_json_is_empty_without_lrc_fallback() {
        assert!(parse_synced_lyrics(r#"{"lyrics":"[00:01.00] hidden"}"#).is_empty());
    }

    #[test]
    fn lrc_requires_fractional_seconds_and_keeps_source_indexes() {
        let lines = parse_synced_lyrics("[00:01] no\nmetadata [02:03.25] yes\n[01:2.] no");
        assert_eq!(
            lines,
            vec![LyricLine {
                time: 123.25,
                text: "yes".into(),
                index: 1
            }]
        );
    }
    #[test]
    fn genius_blocks_and_longest_non_overlapping_annotations() {
        let result = prepare_genius_lyrics(
            "One\r\n\r\nTwo  \nThree",
            &[
                LyricAnnotation {
                    id: "short".into(),
                    fragment: "one".into(),
                    ..Default::default()
                },
                LyricAnnotation {
                    id: "long".into(),
                    fragment: "one\r\n\r\nTwo".into(),
                    ..Default::default()
                },
            ],
            " https://genius.com/x ",
        );
        assert_eq!(result.blocks, vec!["One", "Two\nThree"]);
        assert_eq!(result.line_block_indexes, vec![0, -1, 1, 1]);
        assert_eq!(result.ranges, vec![(0, 8, "long".into())]);
        assert_eq!(result.url, "https://genius.com/x");
    }

    #[test]
    fn genius_annotation_matching_is_case_insensitive_and_repeated() {
        let result = prepare_genius_lyrics(
            "Echo echo ECHO",
            &[LyricAnnotation {
                id: " 7 ".into(),
                fragment: " ECHO ".into(),
                ..Default::default()
            }],
            "",
        );
        assert_eq!(
            result.ranges,
            vec![(0, 4, "7".into()), (5, 9, "7".into()), (10, 14, "7".into())]
        );
    }

    #[test]
    fn genius_ignores_empty_annotations_and_marks_empty_lines() {
        let result = prepare_genius_lyrics(
            "\nVerse\n\n",
            &[LyricAnnotation {
                id: String::new(),
                fragment: "Verse".into(),
                ..Default::default()
            }],
            "  ",
        );
        assert_eq!(result.blocks, vec!["Verse"]);
        assert_eq!(result.line_block_indexes, vec![-1, 0, -1, -1]);
        assert!(result.ranges.is_empty());
    }

    #[test]
    fn lyric_text_helpers_match_context_menu_derivation() {
        let lines = vec!["One  ".into(), "Two".into(), "".into(), "Four".into()];
        assert_eq!(lyric_block_text(&lines, 1), "One\nTwo");
        assert_eq!(lyric_full_text(&lines), "One\nTwo\n\nFour");
        assert_eq!(lyric_block_text(&lines, 2), "");
    }

    #[test]
    fn genius_line_fragments_map_ranges_without_losing_plain_text() {
        let prepared = prepare_genius_lyrics(
            "One two\nThree",
            &[LyricAnnotation {
                id: "1".into(),
                fragment: "one two".into(),
                ..Default::default()
            }],
            "",
        );
        assert_eq!(
            genius_line_fragments(&prepared, 0),
            vec![LyricFragment {
                text: "One two".into(),
                annotation_id: Some("1".into()),
            }]
        );
        assert_eq!(
            genius_line_fragments(&prepared, 1),
            vec![LyricFragment {
                text: "Three".into(),
                annotation_id: None,
            }]
        );
    }

    #[test]
    fn genius_line_fragments_trim_only_the_complete_line_boundaries() {
        let prepared = prepare_genius_lyrics(
            "   before marked after   ",
            &[LyricAnnotation {
                id: "1".into(),
                fragment: "marked".into(),
                ..Default::default()
            }],
            "",
        );

        assert_eq!(
            genius_line_fragments(&prepared, 0),
            vec![
                LyricFragment {
                    text: "before ".into(),
                    annotation_id: None,
                },
                LyricFragment {
                    text: "marked".into(),
                    annotation_id: Some("1".into()),
                },
                LyricFragment {
                    text: " after".into(),
                    annotation_id: None,
                },
            ]
        );
    }

    #[test]
    fn genius_line_fragments_drop_empty_boundary_fragments_but_keep_inner_spaces() {
        let prepared = prepare_genius_lyrics(
            "  first   second  ",
            &[
                LyricAnnotation {
                    id: "1".into(),
                    fragment: "first".into(),
                    ..Default::default()
                },
                LyricAnnotation {
                    id: "2".into(),
                    fragment: "second".into(),
                    ..Default::default()
                },
            ],
            "",
        );

        assert_eq!(
            genius_line_fragments(&prepared, 0),
            vec![
                LyricFragment {
                    text: "first".into(),
                    annotation_id: Some("1".into()),
                },
                LyricFragment {
                    text: "   ".into(),
                    annotation_id: None,
                },
                LyricFragment {
                    text: "second".into(),
                    annotation_id: Some("2".into()),
                },
            ]
        );
    }

    #[test]
    fn genius_identical_fragments_share_one_range_with_every_id() {
        let prepared = prepare_genius_lyrics(
            "[Produced by Diplo & Skrillex]",
            &[
                LyricAnnotation {
                    id: "8506070".into(),
                    fragment: "[Produced by Diplo & Skrillex]".into(),
                    ids: vec!["8506070".into(), "7274811".into()],
                },
                LyricAnnotation {
                    id: "8505909".into(),
                    fragment: "[Produced by Diplo & Skrillex]".into(),
                    ids: vec!["8505909".into(), "7274811".into()],
                },
            ],
            "",
        );
        assert_eq!(prepared.ranges.len(), 1);
        assert_eq!(prepared.ranges[0].2, "8506070,7274811,8505909");
        assert_eq!(
            genius_line_fragments(&prepared, 0),
            vec![LyricFragment {
                text: "[Produced by Diplo & Skrillex]".into(),
                annotation_id: Some("8506070,7274811,8505909".into()),
            }]
        );
    }
}
