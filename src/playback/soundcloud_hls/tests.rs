use super::*;

impl DownloadOutput for tokio::io::Sink {
    fn set_total_hint(&mut self, _total: Option<u64>) {}
}

fn base_url() -> Url {
    Url::parse(
        "https://playback.media-streaming.soundcloud.cloud/path/playlist.m3u8?signature=redacted",
    )
    .unwrap()
}

fn valid_manifest() -> &'static str {
    "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:10\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MAP:URI=\"init.mp4?signature=redacted\"\n#EXTINF:10.0,\nsegment000.m4s?signature=redacted\n#EXTINF:4.0,\nsegment001.m4s?signature=redacted\n#EXT-X-ENDLIST\n"
}

fn mp4_box(kind: &[u8; 4], body: &[u8], extended: bool) -> Vec<u8> {
    let size = if extended {
        16 + body.len()
    } else {
        8 + body.len()
    };
    let mut output = Vec::with_capacity(size);
    if extended {
        output.extend_from_slice(&1_u32.to_be_bytes());
    } else {
        output.extend_from_slice(
            &u32::try_from(size)
                .expect("test box must fit a 32-bit size")
                .to_be_bytes(),
        );
    }
    output.extend_from_slice(kind);
    if extended {
        output.extend_from_slice(&(size as u64).to_be_bytes());
    }
    output.extend_from_slice(body);
    output
}

fn test_fragment(
    sequence_number: u32,
    decode_times: &[(u8, u64)],
    extended: bool,
    mdat_payload: &[u8],
) -> Vec<u8> {
    let decode_times = decode_times
        .iter()
        .enumerate()
        .map(|(index, &(version, value))| (index as u32 + 1, version, value))
        .collect::<Vec<_>>();
    test_fragment_with_tracks(sequence_number, &decode_times, extended, None, mdat_payload)
}

fn test_fragment_with_sidx(
    sequence_number: u32,
    decode_times: &[(u8, u64)],
    extended: bool,
    sidx: Option<Vec<u8>>,
    mdat_payload: &[u8],
) -> Vec<u8> {
    let decode_times = decode_times
        .iter()
        .enumerate()
        .map(|(index, &(version, value))| (index as u32 + 1, version, value))
        .collect::<Vec<_>>();
    test_fragment_with_tracks(sequence_number, &decode_times, extended, sidx, mdat_payload)
}

fn test_fragment_with_tracks(
    sequence_number: u32,
    decode_times: &[(u32, u8, u64)],
    extended: bool,
    sidx: Option<Vec<u8>>,
    mdat_payload: &[u8],
) -> Vec<u8> {
    let mut mfhd_body = vec![0, 0, 0, 0];
    mfhd_body.extend_from_slice(&sequence_number.to_be_bytes());
    let mfhd = mp4_box(b"mfhd", &mfhd_body, extended);

    let mut moof_body = mfhd;
    for &(track_id, version, value) in decode_times {
        let mut tfhd_body = vec![0, 0, 0, 0];
        tfhd_body.extend_from_slice(&track_id.to_be_bytes());
        let tfhd = mp4_box(b"tfhd", &tfhd_body, extended);
        let mut tfdt_body = vec![version, 0, 0, 0];
        match version {
            0 => tfdt_body.extend_from_slice(
                &u32::try_from(value)
                    .expect("v0 test decode time must fit")
                    .to_be_bytes(),
            ),
            1 => tfdt_body.extend_from_slice(&value.to_be_bytes()),
            _ => panic!("unsupported test tfdt version"),
        }
        let mut traf_body = tfhd;
        traf_body.extend(mp4_box(b"tfdt", &tfdt_body, extended));
        let traf = mp4_box(b"traf", &traf_body, extended);
        moof_body.extend(traf);
    }
    let moof = mp4_box(b"moof", &moof_body, extended);
    let mdat = mp4_box(b"mdat", mdat_payload, false);
    let mut output = sidx.unwrap_or_default();
    output.extend(moof);
    output.extend(mdat);
    output
}

fn test_fragment_without_tfhd(sequence_number: u32, decode_time: u64) -> Vec<u8> {
    let mut mfhd_body = vec![0, 0, 0, 0];
    mfhd_body.extend_from_slice(&sequence_number.to_be_bytes());
    let mfhd = mp4_box(b"mfhd", &mfhd_body, false);
    let mut tfdt_body = vec![1, 0, 0, 0];
    tfdt_body.extend_from_slice(&decode_time.to_be_bytes());
    let traf = mp4_box(b"traf", &mp4_box(b"tfdt", &tfdt_body, false), false);
    let mut moof_body = mfhd;
    moof_body.extend(traf);
    let mut output = mp4_box(b"moof", &moof_body, false);
    output.extend(mp4_box(b"mdat", b"payload", false));
    output
}

fn test_sidx(
    version: u8,
    reference_id: u32,
    timescale: u32,
    earliest_presentation_time: u64,
    first_offset: u64,
    references: &[(u32, u32, u32)],
    extended: bool,
) -> Vec<u8> {
    let mut body = vec![version, 0, 0, 0];
    body.extend_from_slice(&reference_id.to_be_bytes());
    body.extend_from_slice(&timescale.to_be_bytes());
    match version {
        0 => {
            body.extend_from_slice(
                &u32::try_from(earliest_presentation_time)
                    .expect("v0 test sidx time must fit")
                    .to_be_bytes(),
            );
            body.extend_from_slice(
                &u32::try_from(first_offset)
                    .expect("v0 test sidx offset must fit")
                    .to_be_bytes(),
            );
        }
        1 => {
            body.extend_from_slice(&earliest_presentation_time.to_be_bytes());
            body.extend_from_slice(&first_offset.to_be_bytes());
        }
        _ => panic!("unsupported test sidx version"),
    }
    body.extend_from_slice(&0_u16.to_be_bytes());
    body.extend_from_slice(
        &u16::try_from(references.len())
            .expect("test sidx reference count must fit")
            .to_be_bytes(),
    );
    for &(reference, duration, sap) in references {
        body.extend_from_slice(&reference.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&sap.to_be_bytes());
    }
    mp4_box(b"sidx", &body, extended)
}

fn normalize_for_test(
    rebaser: &mut FragmentTimelineRebaser,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, PlaybackDownloadError> {
    rebaser.normalize(bytes, &CancellationToken::new())
}

#[test]
fn parses_simple_vod_playlist_and_preserves_signed_query() {
    let parsed = parse_manifest(&base_url(), valid_manifest()).unwrap();
    assert_eq!(parsed.segments.len(), 2);
    assert_eq!(parsed.init_url.path(), "/path/init.mp4");
    assert_eq!(parsed.segments[0].path(), "/path/segment000.m4s");
    assert_eq!(parsed.segments[0].query(), Some("signature=redacted"));
    assert_eq!(parsed.buffered_fraction(1), Some(10.0 / 14.0));
    assert_eq!(parsed.buffered_fraction(2), Some(1.0));
}

#[test]
fn init_fragment_cache_is_shared_by_descriptor_clones_and_written_once() {
    let parsed = parse_manifest(&base_url(), valid_manifest()).unwrap();
    assert!(parsed.cached_init_fragment().is_none());

    parsed.cache_init_fragment(vec![1, 2, 3]);
    parsed.cache_init_fragment(vec![9, 9, 9]);
    let clone = parsed.clone();

    assert_eq!(
        parsed.cached_init_fragment().as_deref().map(Vec::as_slice),
        Some(&[1, 2, 3][..])
    );
    assert_eq!(
        clone.cached_init_fragment().as_deref().map(Vec::as_slice),
        Some(&[1, 2, 3][..])
    );
}

#[test]
fn seek_target_uses_boundaries_and_clamps_to_the_playlist() {
    let parsed = parse_manifest(&base_url(), valid_manifest()).unwrap();
    assert_eq!(
        parsed.seek_target(Duration::ZERO),
        HlsSeekTarget {
            index: 0,
            segment_start: Duration::ZERO,
            offset: Duration::ZERO,
        }
    );
    assert_eq!(
        parsed.seek_target(Duration::from_secs(10)),
        HlsSeekTarget {
            index: 1,
            segment_start: Duration::from_secs(10),
            offset: Duration::ZERO,
        }
    );
    assert_eq!(
        parsed.seek_target(Duration::from_secs(12)),
        HlsSeekTarget {
            index: 1,
            segment_start: Duration::from_secs(10),
            offset: Duration::from_secs(2),
        }
    );
    assert_eq!(parsed.seek_target(Duration::from_secs(60)).index, 1);
    assert_eq!(
        parsed.seek_target(Duration::from_secs(60)).offset,
        Duration::from_secs(4)
    );
}

#[test]
fn seek_target_scales_to_a_forty_minute_timeline() {
    let durations = vec![Duration::from_secs(4); 600];
    let mut starts = Vec::with_capacity(durations.len());
    let mut total = Duration::ZERO;
    for duration in &durations {
        starts.push(total);
        total += *duration;
    }
    let descriptor = HlsDescriptor {
        manifest_url: base_url(),
        init_url: base_url(),
        segments: (0..durations.len())
            .map(|index| base_url().join(&format!("segment{index}.m4s")).unwrap())
            .collect(),
        segment_durations: durations,
        segment_starts: starts,
        total_duration: Some(total),
        init_fragment: Arc::new(OnceLock::new()),
    };
    let target = descriptor.seek_target(Duration::from_secs(2_399));
    assert_eq!(target.index, 599);
    assert_eq!(target.segment_start, Duration::from_secs(2_396));
    assert_eq!(target.offset, Duration::from_secs(3));
}

#[test]
fn ordered_fragment_indices_are_written_only_after_prior_fragments() {
    let mut ready = BTreeMap::from([(3_usize, b"third".to_vec()), (1_usize, b"first".to_vec())]);
    let mut assembled = Vec::new();
    let mut next = 1_usize;
    while let Some(fragment) = ready.remove(&next) {
        assembled.extend(fragment);
        next += 1;
    }
    assert_eq!(assembled, b"first");
    ready.insert(2, b"second".to_vec());
    while let Some(fragment) = ready.remove(&next) {
        assembled.extend(fragment);
        next += 1;
    }
    assert_eq!(assembled, b"firstsecondthird");
}

#[test]
fn rebases_v1_sequence_and_decode_time_from_selected_suffix_fragment() {
    let mut rebaser = FragmentTimelineRebaser::default();
    let selected = test_fragment(41, &[(1, 900_000)], false, b"selected");
    let later = test_fragment(44, &[(1, 900_420)], false, b"later");

    let selected = normalize_for_test(&mut rebaser, selected).unwrap();
    let later = normalize_for_test(&mut rebaser, later).unwrap();

    assert_eq!(
        parse_fragment_timeline(&selected).unwrap().metadata,
        FragmentTimelineMetadata {
            sequence_number: 1,
            decode_times: BTreeMap::from([(1, 0)]),
            sidx: None,
        }
    );
    assert_eq!(
        parse_fragment_timeline(&later).unwrap().metadata,
        FragmentTimelineMetadata {
            sequence_number: 4,
            decode_times: BTreeMap::from([(1, 420)]),
            sidx: None,
        }
    );
}

#[test]
fn rebases_v0_decode_time_without_widening_the_field() {
    let mut rebaser = FragmentTimelineRebaser::default();
    let selected = test_fragment(7, &[(0, 65_000)], false, b"selected");
    let later = test_fragment(8, &[(0, 65_512)], false, b"later");

    let selected = normalize_for_test(&mut rebaser, selected).unwrap();
    let later = normalize_for_test(&mut rebaser, later).unwrap();

    assert_eq!(
        parse_fragment_timeline(&selected).unwrap().metadata,
        FragmentTimelineMetadata {
            sequence_number: 1,
            decode_times: BTreeMap::from([(1, 0)]),
            sidx: None,
        }
    );
    let later_metadata = parse_fragment_timeline(&later).unwrap();
    assert_eq!(
        later_metadata.metadata.decode_times,
        BTreeMap::from([(1, 512)])
    );
    assert_eq!(later_metadata.metadata.decode_times[&1], 512);
    assert_eq!(later_metadata.decode_times[0].width, 4);
}

#[test]
fn rebases_v1_sidx_time_and_preserves_first_offset_and_references() {
    let references = [(0x0000_0123, 44_100, 0x9000_0001)];
    let selected_sidx = test_sidx(1, 7, 44_100, 2_205_696, 321, &references, false);
    let later_sidx = test_sidx(1, 7, 44_100, 2_206_140, 321, &references, false);
    let selected = test_fragment_with_sidx(
        50,
        &[(1, 2_205_696)],
        false,
        Some(selected_sidx),
        b"selected",
    );
    let later = test_fragment_with_sidx(51, &[(1, 2_206_140)], false, Some(later_sidx), b"later");
    let mut rebaser = FragmentTimelineRebaser::default();

    let selected = normalize_for_test(&mut rebaser, selected).unwrap();
    let later = normalize_for_test(&mut rebaser, later).unwrap();
    let selected_sidx = parse_fragment_timeline(&selected)
        .unwrap()
        .sidx
        .expect("selected fragment must retain sidx");
    let later_sidx = parse_fragment_timeline(&later)
        .unwrap()
        .sidx
        .expect("later fragment must retain sidx");
    assert_eq!(selected_sidx.metadata.earliest_presentation_time, 0);
    assert_eq!(later_sidx.metadata.earliest_presentation_time, 444);

    let original = test_fragment_with_sidx(
        50,
        &[(1, 2_205_696)],
        false,
        Some(test_sidx(1, 7, 44_100, 2_205_696, 321, &references, false)),
        b"selected",
    );
    let original_box = parse_iso_bmff_box(&original, 0, original.len()).unwrap();
    let after_earliest = selected_sidx.earliest_offset + selected_sidx.width;
    assert_eq!(
        &original[after_earliest..original_box.end],
        &selected[after_earliest..original_box.end]
    );
}

#[test]
fn rebases_v0_sidx_time_and_keeps_its_width() {
    let references = [(0x8000_0010, 22_050, 0x7000_0002)];
    let selected_sidx = test_sidx(0, 9, 44_100, 65_000, 17, &references, false);
    let later_sidx = test_sidx(0, 9, 44_100, 65_512, 17, &references, false);
    let selected =
        test_fragment_with_sidx(60, &[(0, 65_000)], false, Some(selected_sidx), b"selected");
    let later = test_fragment_with_sidx(61, &[(0, 65_512)], false, Some(later_sidx), b"later");
    let mut rebaser = FragmentTimelineRebaser::default();

    let selected = normalize_for_test(&mut rebaser, selected).unwrap();
    let later = normalize_for_test(&mut rebaser, later).unwrap();
    let selected_sidx = parse_fragment_timeline(&selected)
        .unwrap()
        .sidx
        .expect("selected fragment must retain sidx");
    let later_sidx = parse_fragment_timeline(&later)
        .unwrap()
        .sidx
        .expect("later fragment must retain sidx");
    assert_eq!(selected_sidx.metadata.earliest_presentation_time, 0);
    assert_eq!(later_sidx.metadata.earliest_presentation_time, 512);
    assert_eq!(later_sidx.width, 4);
}

#[test]
fn traverses_extended_size_sidx_and_rebases_its_timeline() {
    let sidx = test_sidx(1, 10, 44_100, 1_000_000, 29, &[], true);
    let later_sidx = test_sidx(1, 10, 44_100, 1_000_001, 29, &[], true);
    let selected = test_fragment_with_sidx(70, &[(1, 1_000_000)], true, Some(sidx), b"selected");
    let later = test_fragment_with_sidx(71, &[(1, 1_000_001)], true, Some(later_sidx), b"later");
    let mut rebaser = FragmentTimelineRebaser::default();

    normalize_for_test(&mut rebaser, selected).unwrap();
    let later = normalize_for_test(&mut rebaser, later).unwrap();
    assert_eq!(
        parse_fragment_timeline(&later)
            .unwrap()
            .sidx
            .unwrap()
            .metadata
            .earliest_presentation_time,
        1
    );
}

#[test]
fn rejects_sidx_presence_and_identity_changes_in_a_seek_suffix() {
    let with_sidx = test_fragment_with_sidx(
        80,
        &[(1, 100)],
        false,
        Some(test_sidx(1, 11, 44_100, 100, 0, &[], false)),
        b"selected",
    );
    let without_sidx = test_fragment(81, &[(1, 101)], false, b"later");
    let mut rebaser = FragmentTimelineRebaser::default();
    normalize_for_test(&mut rebaser, with_sidx).unwrap();
    let error = normalize_for_test(&mut rebaser, without_sidx).unwrap_err();
    assert!(error.message.contains("sidx presence changed"));

    let without_sidx = test_fragment(90, &[(1, 100)], false, b"selected");
    let with_sidx = test_fragment_with_sidx(
        91,
        &[(1, 101)],
        false,
        Some(test_sidx(1, 11, 44_100, 101, 0, &[], false)),
        b"later",
    );
    let mut rebaser = FragmentTimelineRebaser::default();
    normalize_for_test(&mut rebaser, without_sidx).unwrap();
    let error = normalize_for_test(&mut rebaser, with_sidx).unwrap_err();
    assert!(error.message.contains("sidx presence changed"));

    let selected = test_fragment_with_sidx(
        100,
        &[(1, 100)],
        false,
        Some(test_sidx(1, 12, 44_100, 100, 0, &[], false)),
        b"selected",
    );
    let changed_identity = test_fragment_with_sidx(
        101,
        &[(1, 101)],
        false,
        Some(test_sidx(1, 13, 44_100, 101, 0, &[], false)),
        b"later",
    );
    let mut rebaser = FragmentTimelineRebaser::default();
    normalize_for_test(&mut rebaser, selected).unwrap();
    let error = normalize_for_test(&mut rebaser, changed_identity).unwrap_err();
    assert!(error.message.contains("sidx identity or version changed"));
}

#[test]
fn traverses_extended_size_boxes_and_rebases_later_fragment() {
    let mut rebaser = FragmentTimelineRebaser::default();
    let selected = test_fragment(100, &[(1, 1_000_000)], true, b"selected");
    let later = test_fragment(101, &[(1, 1_001_024)], true, b"later");

    normalize_for_test(&mut rebaser, selected).unwrap();
    let later = normalize_for_test(&mut rebaser, later).unwrap();

    assert_eq!(
        parse_fragment_timeline(&later).unwrap().metadata,
        FragmentTimelineMetadata {
            sequence_number: 2,
            decode_times: BTreeMap::from([(1, 1_024)]),
            sidx: None,
        }
    );
}

#[test]
fn does_not_rewrite_mdat_bytes_that_resemble_timeline_boxes() {
    let decoy = test_fragment(
        12,
        &[(1, 800)],
        false,
        &[
            0, 0, 0, 16, b'm', b'f', b'h', b'd', 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 16, b't', b'f',
            b'd', b't', 1, 0, 0, 0, 0, 0, 0, 2,
        ],
    );
    let mut rebaser = FragmentTimelineRebaser::default();
    let normalized = normalize_for_test(&mut rebaser, decoy.clone()).unwrap();

    let mdat_start = decoy
        .windows(4)
        .position(|window| window == b"mdat")
        .expect("test fragment must contain mdat")
        - 4;
    assert_eq!(&normalized[mdat_start..], &decoy[mdat_start..]);
    assert_eq!(
        parse_fragment_timeline(&normalized).unwrap().metadata,
        FragmentTimelineMetadata {
            sequence_number: 1,
            decode_times: BTreeMap::from([(1, 0)]),
            sidx: None,
        }
    );
}

#[test]
fn rejects_malformed_and_underflowing_suffix_metadata() {
    let mut malformed_rebaser = FragmentTimelineRebaser::default();
    let malformed = mp4_box(b"mdat", b"not a moof", false);
    let error = normalize_for_test(&mut malformed_rebaser, malformed).unwrap_err();
    assert!(error.message.contains("invalid ISO-BMFF timeline metadata"));

    let mut underflow_rebaser = FragmentTimelineRebaser::default();
    let selected = test_fragment(20, &[(1, 10_000)], false, b"selected");
    let earlier = test_fragment(21, &[(1, 9_999)], false, b"earlier");
    normalize_for_test(&mut underflow_rebaser, selected).unwrap();
    let error = normalize_for_test(&mut underflow_rebaser, earlier).unwrap_err();
    assert!(error.message.contains("decode times are not monotonic"));

    let mut inconsistent_rebaser = FragmentTimelineRebaser::default();
    let selected = test_fragment(30, &[(1, 10_000)], false, b"selected");
    let changed_tracks = test_fragment_with_tracks(
        31,
        &[(1, 1, 10_500), (1, 1, 10_500)],
        false,
        None,
        b"duplicate track IDs",
    );
    normalize_for_test(&mut inconsistent_rebaser, selected).unwrap();
    let error = normalize_for_test(&mut inconsistent_rebaser, changed_tracks).unwrap_err();
    assert!(error.message.contains("duplicate tfhd track IDs"));
}

#[test]
fn normalized_nonzero_suffix_stays_in_order_and_preserves_deltas() {
    let mut rebaser = FragmentTimelineRebaser::default();
    let fragments = [
        test_fragment(90, &[(1, 500_000)], false, b"six"),
        test_fragment(91, &[(1, 500_480)], false, b"seven"),
        test_fragment(92, &[(1, 501_120)], false, b"eight"),
    ];
    let normalized = fragments
        .iter()
        .map(|fragment| normalize_for_test(&mut rebaser, fragment.to_vec()).unwrap())
        .collect::<Vec<_>>();
    let metadata = normalized
        .iter()
        .map(|fragment| parse_fragment_timeline(fragment).unwrap().metadata)
        .collect::<Vec<_>>();

    assert_eq!(
        metadata
            .iter()
            .map(|fragment| fragment.sequence_number)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        metadata
            .iter()
            .map(|fragment| fragment.decode_times[&1])
            .collect::<Vec<_>>(),
        vec![0, 480, 1_120]
    );
}

#[test]
fn compares_reordered_tracks_by_track_id() {
    let selected =
        test_fragment_with_tracks(10, &[(10, 1, 100), (20, 1, 200)], false, None, b"selected");
    let later = test_fragment_with_tracks(11, &[(20, 1, 250), (10, 1, 150)], false, None, b"later");
    let mut rebaser = FragmentTimelineRebaser::default();
    normalize_for_test(&mut rebaser, selected).unwrap();
    let later = normalize_for_test(&mut rebaser, later).unwrap();
    let metadata = parse_fragment_timeline(&later).unwrap().metadata;

    assert_eq!(metadata.decode_times, BTreeMap::from([(10, 50), (20, 50)]));
    assert_eq!(
        parse_fragment_timeline(&later)
            .unwrap()
            .decode_times
            .iter()
            .map(|field| field.value)
            .collect::<Vec<_>>(),
        vec![50, 50]
    );
}

#[test]
fn rejects_duplicate_missing_and_changed_track_ids() {
    let selected =
        test_fragment_with_tracks(20, &[(10, 1, 100), (20, 1, 200)], false, None, b"selected");
    let changed =
        test_fragment_with_tracks(21, &[(10, 1, 150), (30, 1, 250)], false, None, b"changed");
    let mut rebaser = FragmentTimelineRebaser::default();
    normalize_for_test(&mut rebaser, selected).unwrap();
    let error = normalize_for_test(&mut rebaser, changed).unwrap_err();
    assert!(error.message.contains("track IDs changed"));

    let duplicate =
        test_fragment_with_tracks(22, &[(10, 1, 150), (10, 1, 250)], false, None, b"duplicate");
    let error = normalize_for_test(&mut rebaser, duplicate).unwrap_err();
    assert!(error.message.contains("duplicate tfhd track IDs"));

    let missing = test_fragment_without_tfhd(23, 300);
    let error = normalize_for_test(&mut rebaser, missing).unwrap_err();
    assert!(error.message.contains("traf does not contain a tfhd"));
}

#[test]
fn rejects_duplicate_sequence_numbers_after_the_selected_fragment() {
    let selected = test_fragment(30, &[(1, 100)], false, b"selected");
    let duplicate = test_fragment(30, &[(1, 200)], false, b"duplicate");
    let mut rebaser = FragmentTimelineRebaser::default();
    normalize_for_test(&mut rebaser, selected).unwrap();
    let error = normalize_for_test(&mut rebaser, duplicate).unwrap_err();
    assert!(error.message.contains("not strictly increasing"));
}

#[test]
fn requires_a_nonempty_structurally_bounded_top_level_mdat() {
    let missing = test_fragment(40, &[(1, 100)], false, b"payload");
    let mdat_start = missing
        .windows(4)
        .position(|window| window == b"mdat")
        .expect("test fragment must contain mdat")
        - 4;
    let missing = missing[..mdat_start].to_vec();
    let mut rebaser = FragmentTimelineRebaser::default();
    let error = normalize_for_test(&mut rebaser, missing).unwrap_err();
    assert!(error.message.contains("nonempty top-level mdat"));

    let empty = test_fragment(41, &[(1, 100)], false, b"");
    let error = normalize_for_test(&mut rebaser, empty).unwrap_err();
    assert!(error.message.contains("nonempty top-level mdat"));

    let mut truncated = test_fragment(42, &[(1, 100)], false, b"payload");
    let mdat_start = truncated
        .windows(4)
        .position(|window| window == b"mdat")
        .expect("test fragment must contain mdat")
        - 4;
    let claimed_size = u32::try_from(truncated.len() - mdat_start + 1)
        .expect("test fragment must fit a 32-bit size");
    truncated[mdat_start..mdat_start + 4].copy_from_slice(&claimed_size.to_be_bytes());
    let error = normalize_for_test(&mut rebaser, truncated).unwrap_err();
    assert!(error.message.contains("extends past its parent"));
}

#[test]
fn normalize_honors_cancellation_before_parsing() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut rebaser = FragmentTimelineRebaser::default();
    let error = rebaser
        .normalize(
            test_fragment(50, &[(1, 100)], false, b"payload"),
            &cancellation,
        )
        .unwrap_err();
    assert_eq!(error.message, "Playback request cancelled");
}

#[test]
fn fetch_window_counts_pending_and_ready_segments() {
    assert_eq!(fetch_window_slots(0, 0), MEDIA_CONCURRENCY);
    assert_eq!(fetch_window_slots(2, 1), 0);
    assert_eq!(fetch_window_slots(0, 1), MEDIA_CONCURRENCY - 1);
    assert_eq!(fetch_window_slots(usize::MAX, 1), 0);
}

#[test]
fn rejects_master_playlists_and_encrypted_media() {
    for body in [
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nchild.m3u8\n",
        "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-KEY:METHOD=AES-128\n#EXT-X-ENDLIST\n",
    ] {
        assert!(parse_manifest(&base_url(), body).is_err());
    }
}

#[test]
fn requires_vod_endlist_init_map_and_segments() {
    for body in [
        "#EXTM3U\n#EXT-X-ENDLIST\n",
        "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-ENDLIST\n",
        "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:1,\nsegment.m4s\n",
    ] {
        assert!(parse_manifest(&base_url(), body).is_err());
    }
}

#[test]
fn rejects_non_soundcloud_playback_hosts() {
    let invalid = Url::parse("https://evil.example.test/playlist.m3u8").unwrap();
    assert!(validate_playback_url(&invalid).is_err());
    let body = valid_manifest().replace("segment001.m4s", "https://evil.example.test/segment.m4s");
    assert!(parse_manifest(&base_url(), &body).is_err());
}

#[test]
fn rejects_oversized_manifests_and_segment_lists() {
    let oversized = format!("#EXTM3U\n{}", "x".repeat(MAX_MANIFEST_BYTES));
    assert!(parse_manifest(&base_url(), &oversized).is_err());

    let mut too_many_segments =
        String::from("#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MAP:URI=\"init.mp4\"\n");
    for index in 0..=MAX_SEGMENTS {
        too_many_segments.push_str(&format!("#EXTINF:1,\nsegment{index}.m4s\n"));
    }
    too_many_segments.push_str("#EXT-X-ENDLIST\n");
    assert!(parse_manifest(&base_url(), &too_many_segments).is_err());
}

#[tokio::test]
async fn download_checks_cancellation_before_fetching_media() {
    let descriptor = HlsDescriptor {
            manifest_url: base_url(),
            init_url: Url::parse(
                "https://playback.media-streaming.soundcloud.cloud/path/init.mp4?signature=redacted",
            )
            .unwrap(),
            segments: vec![Url::parse(
                "https://playback.media-streaming.soundcloud.cloud/path/segment.m4s?signature=redacted",
            )
            .unwrap()],
            segment_durations: vec![Duration::from_secs(1)],
            segment_starts: vec![Duration::ZERO],
            total_duration: Some(Duration::from_secs(1)),
            init_fragment: Arc::new(OnceLock::new()),
        };
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut output = tokio::io::sink();
    let result = download(
        &Client::new(),
        &descriptor,
        &mut output,
        &cancellation,
        None,
        1024,
        "test-agent",
    )
    .await;
    assert_eq!(
        result.expect_err("cancelled download must fail").message,
        "Playback request cancelled"
    );
}
