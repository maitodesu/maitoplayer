use contracts::{AppErrorV1, SubtitleCueV1, TimestampUs, error_codes};

const MAX_PADDING_US: TimestampUs = 60_000_000;
const MAX_PROFILE_LEN: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipPolicy {
    pub leading_padding_us: TimestampUs,
    pub trailing_padding_us: TimestampUs,
    pub audio_profile: String,
    pub image_profile: String,
}

impl Default for ClipPolicy {
    fn default() -> Self {
        Self {
            leading_padding_us: 250_000,
            trailing_padding_us: 500_000,
            audio_profile: "mp3-128k-mono-v1".into(),
            image_profile: "jpeg-q2-v1".into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetSpec {
    pub audio_start_us: TimestampUs,
    pub audio_end_us: TimestampUs,
    pub frame_us: TimestampUs,
    pub profile: String,
}

impl ClipPolicy {
    pub fn compute(
        &self,
        cue: &SubtitleCueV1,
        observed_source_time_us: TimestampUs,
        media_duration_us: TimestampUs,
    ) -> Result<AssetSpec, AppErrorV1> {
        validate_padding(self.leading_padding_us)?;
        validate_padding(self.trailing_padding_us)?;
        validate_profile(&self.audio_profile)?;
        validate_profile(&self.image_profile)?;
        if cue.start_us < 0 || cue.end_us <= cue.start_us || media_duration_us <= 0 {
            return Err(policy_error("Cue or media duration was invalid."));
        }
        let audio_start_us = cue.start_us.saturating_sub(self.leading_padding_us).max(0);
        let audio_end_us = cue
            .end_us
            .saturating_add(self.trailing_padding_us)
            .min(media_duration_us);
        if audio_end_us <= audio_start_us {
            return Err(policy_error("Audio clip became empty after clamping."));
        }
        let midpoint = cue.start_us.saturating_add((cue.end_us - cue.start_us) / 2);
        let frame_us = if (cue.start_us..cue.end_us).contains(&observed_source_time_us) {
            observed_source_time_us
        } else {
            midpoint
        }
        .clamp(0, media_duration_us.saturating_sub(1));
        Ok(AssetSpec {
            audio_start_us,
            audio_end_us,
            frame_us,
            profile: format!("{}+{}", self.audio_profile, self.image_profile),
        })
    }
}

fn validate_padding(padding_us: TimestampUs) -> Result<(), AppErrorV1> {
    if !(0..=MAX_PADDING_US).contains(&padding_us) {
        return Err(policy_error(
            "Clip padding must be between zero and sixty seconds.",
        ));
    }
    Ok(())
}

fn validate_profile(profile: &str) -> Result<(), AppErrorV1> {
    if profile.is_empty()
        || profile.len() > MAX_PROFILE_LEN
        || !profile
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(policy_error(
            "The extraction profile identifier was invalid.",
        ));
    }
    Ok(())
}

fn policy_error(detail: &str) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::INVALID_REQUEST,
        "A valid media interval could not be created for this subtitle.",
        false,
    )
    .with_diagnostics(detail)
}

#[cfg(test)]
mod tests {
    use contracts::{CueId, SubtitleStyleHintV1};

    use super::*;

    fn cue(start: i64, end: i64) -> SubtitleCueV1 {
        SubtitleCueV1 {
            cue_id: CueId::new("c"),
            start_us: start,
            end_us: end,
            plain_text: "見る".into(),
            source_text: "見る".into(),
            track_order: 0,
            style_hint: SubtitleStyleHintV1::default(),
        }
    }

    #[test]
    fn clamps_padding_and_prefers_paused_time() -> Result<(), AppErrorV1> {
        let policy = ClipPolicy::default();
        let spec = policy.compute(&cue(100_000, 1_000_000), 900_000, 1_200_000)?;
        assert_eq!(spec.audio_start_us, 0);
        assert_eq!(spec.audio_end_us, 1_200_000);
        assert_eq!(spec.frame_us, 900_000);
        Ok(())
    }

    #[test]
    fn uses_midpoint_when_observation_is_outside_cue() -> Result<(), AppErrorV1> {
        let spec =
            ClipPolicy::default().compute(&cue(1_000_000, 3_000_000), 7_000_000, 10_000_000)?;
        assert_eq!(spec.frame_us, 2_000_000);
        Ok(())
    }

    #[test]
    fn rejects_negative_or_excessive_padding() {
        let policy = ClipPolicy {
            leading_padding_us: -1,
            ..ClipPolicy::default()
        };
        assert!(policy.compute(&cue(1, 2), 1, 10).is_err());

        let policy = ClipPolicy {
            leading_padding_us: MAX_PADDING_US + 1,
            ..ClipPolicy::default()
        };
        assert!(policy.compute(&cue(1, 2), 1, 10).is_err());
    }

    #[test]
    fn rejects_unsafe_profile_identifiers() {
        let policy = ClipPolicy {
            audio_profile: "mp3; delete".into(),
            ..ClipPolicy::default()
        };
        assert!(policy.compute(&cue(1, 2), 1, 10).is_err());
    }
}
