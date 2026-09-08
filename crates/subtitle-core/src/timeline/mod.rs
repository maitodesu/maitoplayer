use contracts::{
    AnalysisStateV1, AnalyzedCueV1, MediaSessionId, SubtitleCueV1, SubtitleSourceId,
    SubtitleWindowV1, TimestampUs, TranslationWindowV1,
};
use sha2::{Digest, Sha256};

const DEFAULT_WINDOW_RADIUS_US: i64 = 30_000_000;
const MAX_WINDOW_CUES: usize = 200;

#[derive(Clone, Debug)]
pub struct Timeline {
    cues: Vec<SubtitleCueV1>,
    max_end_tree: Vec<TimestampUs>,
    leaf_count: usize,
}

impl Timeline {
    #[must_use]
    pub fn new(mut cues: Vec<SubtitleCueV1>) -> Self {
        cues.sort_by_key(|cue| (cue.start_us, cue.track_order, cue.end_us));
        let leaf_count = cues.len().next_power_of_two().max(1);
        let mut max_end_tree = vec![TimestampUs::MIN; leaf_count * 2];
        for (index, cue) in cues.iter().enumerate() {
            max_end_tree[leaf_count + index] = cue.end_us;
        }
        for index in (1..leaf_count).rev() {
            max_end_tree[index] = max_end_tree[index * 2].max(max_end_tree[index * 2 + 1]);
        }
        Self {
            cues,
            max_end_tree,
            leaf_count,
        }
    }

    #[must_use]
    pub fn active_at(&self, position_us: TimestampUs) -> Vec<&SubtitleCueV1> {
        let upper = self.cues.partition_point(|cue| cue.start_us <= position_us);
        let mut indexes = Vec::new();
        self.collect_ending_after(
            1,
            0,
            self.leaf_count,
            upper,
            position_us,
            usize::MAX,
            &mut indexes,
        );
        indexes.into_iter().map(|index| &self.cues[index]).collect()
    }

    #[must_use]
    pub fn window(
        &self,
        session_id: MediaSessionId,
        source_id: SubtitleSourceId,
        position_us: TimestampUs,
        revision: u64,
    ) -> SubtitleWindowV1 {
        let start = position_us.saturating_sub(DEFAULT_WINDOW_RADIUS_US).max(0);
        let end = position_us.saturating_add(DEFAULT_WINDOW_RADIUS_US);
        let upper = self.cues.partition_point(|cue| cue.start_us < end);
        let mut indexes = Vec::with_capacity(MAX_WINDOW_CUES + 1);
        self.collect_ending_after(
            1,
            0,
            self.leaf_count,
            upper,
            start,
            MAX_WINDOW_CUES + 1,
            &mut indexes,
        );
        let truncated = indexes.len() > MAX_WINDOW_CUES;
        indexes.truncate(MAX_WINDOW_CUES);
        let cues: Vec<_> = indexes
            .into_iter()
            .map(|index| self.cues[index].clone())
            .map(|cue| AnalyzedCueV1 {
                cue,
                tokens: Vec::new(),
            })
            .collect();
        // Refresh near the edge of the bounded window, not at the end of the
        // last sparse cue. Otherwise a quiet section causes one backend request
        // per animation frame while the same window is still valid.
        let recommended_refresh_at_us = end.saturating_sub(5_000_000).max(position_us);
        let window_id = window_id(&session_id, &source_id, start, end, revision);
        SubtitleWindowV1 {
            window_id,
            revision,
            session_id,
            subtitle_source_id: source_id,
            window_start_us: start,
            window_end_us: end,
            cues,
            recommended_refresh_at_us,
            analysis_state: AnalysisStateV1::Pending,
            warnings: if truncated {
                vec!["Subtitle window was capped at 200 overlapping cues.".into()]
            } else {
                Vec::new()
            },
        }
    }

    /// Returns parsed translation cues without running language analysis.
    /// The range and cap intentionally match the interactive window so both
    /// tracks refresh from the same media-clock cadence.
    #[must_use]
    pub fn translation_window(
        &self,
        session_id: MediaSessionId,
        source_id: SubtitleSourceId,
        position_us: TimestampUs,
        revision: u64,
    ) -> TranslationWindowV1 {
        let start = position_us.saturating_sub(DEFAULT_WINDOW_RADIUS_US).max(0);
        let end = position_us.saturating_add(DEFAULT_WINDOW_RADIUS_US);
        let upper = self.cues.partition_point(|cue| cue.start_us < end);
        let mut indexes = Vec::with_capacity(MAX_WINDOW_CUES + 1);
        self.collect_ending_after(
            1,
            0,
            self.leaf_count,
            upper,
            start,
            MAX_WINDOW_CUES + 1,
            &mut indexes,
        );
        let truncated = indexes.len() > MAX_WINDOW_CUES;
        indexes.truncate(MAX_WINDOW_CUES);
        let cues = indexes
            .into_iter()
            .map(|index| self.cues[index].clone())
            .collect();
        let recommended_refresh_at_us = end.saturating_sub(5_000_000).max(position_us);
        let window_id = window_id(&session_id, &source_id, start, end, revision);
        TranslationWindowV1 {
            window_id,
            revision,
            session_id,
            subtitle_source_id: source_id,
            window_start_us: start,
            window_end_us: end,
            cues,
            recommended_refresh_at_us,
            warnings: if truncated {
                vec!["Translation window was capped at 200 overlapping cues.".into()]
            } else {
                Vec::new()
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_ending_after(
        &self,
        node: usize,
        left: usize,
        right: usize,
        index_end: usize,
        threshold: TimestampUs,
        limit: usize,
        output: &mut Vec<usize>,
    ) {
        if left >= index_end || output.len() >= limit || self.max_end_tree[node] <= threshold {
            return;
        }
        if right - left == 1 {
            if left < self.cues.len() {
                output.push(left);
            }
            return;
        }
        let middle = left + (right - left) / 2;
        self.collect_ending_after(node * 2, left, middle, index_end, threshold, limit, output);
        self.collect_ending_after(
            node * 2 + 1,
            middle,
            right,
            index_end,
            threshold,
            limit,
            output,
        );
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.cues.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }
}

fn window_id(
    session_id: &MediaSessionId,
    source_id: &SubtitleSourceId,
    start: i64,
    end: i64,
    revision: u64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"maitoplayer-window-v1\0");
    hasher.update(session_id.as_str().as_bytes());
    hasher.update(source_id.as_str().as_bytes());
    hasher.update(start.to_le_bytes());
    hasher.update(end.to_le_bytes());
    hasher.update(revision.to_le_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("win_{}", &digest[..24])
}

#[cfg(test)]
mod tests {
    use contracts::{CueId, SubtitleStyleHintV1};

    use super::*;

    fn cue(id: &str, start: i64, end: i64, order: u32) -> SubtitleCueV1 {
        SubtitleCueV1 {
            cue_id: CueId::new(id),
            start_us: start,
            end_us: end,
            plain_text: id.into(),
            source_text: id.into(),
            track_order: order,
            style_hint: SubtitleStyleHintV1::default(),
        }
    }

    #[test]
    fn selects_overlaps_with_half_open_end() {
        let timeline = Timeline::new(vec![cue("a", 0, 20, 0), cue("b", 10, 30, 1)]);
        assert_eq!(timeline.active_at(10).len(), 2);
        assert_eq!(
            timeline
                .active_at(20)
                .iter()
                .map(|cue| cue.cue_id.as_str())
                .collect::<Vec<_>>(),
            vec!["b"]
        );
    }

    #[test]
    fn windows_are_bounded() {
        let cues = (0..500)
            .map(|index| {
                cue(
                    &format!("{index}"),
                    index * 1_000,
                    index * 1_000 + 900,
                    index as u32,
                )
            })
            .collect();
        let window = Timeline::new(cues).window(
            MediaSessionId::new("s"),
            SubtitleSourceId::new("sub"),
            100_000,
            1,
        );
        assert!(window.cues.len() <= MAX_WINDOW_CUES);
        assert_eq!(window.warnings.len(), 1);
    }

    #[test]
    fn sparse_cues_do_not_force_immediate_refresh() {
        let window = Timeline::new(vec![cue("early", 0, 1_000_000, 0)]).window(
            MediaSessionId::new("s"),
            SubtitleSourceId::new("sub"),
            0,
            1,
        );
        assert_eq!(window.recommended_refresh_at_us, 25_000_000);
    }

    #[test]
    fn interval_tree_finds_a_long_overlap_among_expired_cues() {
        let mut cues = vec![cue("long", 0, 1_000_000, 0)];
        cues.extend(
            (1..1_000).map(|index| cue(&index.to_string(), index, index + 1, index as u32)),
        );
        let timeline = Timeline::new(cues);
        assert_eq!(
            timeline
                .active_at(900_000)
                .iter()
                .map(|cue| cue.cue_id.as_str())
                .collect::<Vec<_>>(),
            vec!["long"]
        );
    }

    #[test]
    fn translation_window_is_raw_sorted_and_half_open() {
        let timeline = Timeline::new(vec![
            cue("later", 20, 30, 2),
            cue("first", 10, 20, 1),
            cue("overlap", 15, 25, 0),
        ]);
        let window = timeline.translation_window(
            MediaSessionId::new("session"),
            SubtitleSourceId::new("translation"),
            20,
            7,
        );
        assert_eq!(
            window
                .cues
                .iter()
                .map(|item| item.cue_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "overlap", "later"]
        );
        assert!(!window.cues[0].is_active_at(20));
        assert!(window.cues[1].is_active_at(20));
        assert!(window.cues[2].is_active_at(20));
        assert_eq!(window.revision, 7);
    }

    #[test]
    fn translation_window_caps_pathological_overlap() {
        let cues = (0..=MAX_WINDOW_CUES)
            .map(|index| cue(&format!("cue-{index}"), 0, 60_000_000, index as u32))
            .collect();
        let window = Timeline::new(cues).translation_window(
            MediaSessionId::new("session"),
            SubtitleSourceId::new("translation"),
            10_000_000,
            1,
        );
        assert_eq!(window.cues.len(), MAX_WINDOW_CUES);
        assert_eq!(window.warnings.len(), 1);
    }
}
