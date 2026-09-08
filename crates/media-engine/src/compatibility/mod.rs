use contracts::{
    DecodeTestOutcomeV1, EstimatedCostV1, JobStateV1, MediaStreamKindV1, MediaStreamV1,
    PlaybackCapabilityReportV1, PlaybackPlanKindV1, PlaybackPlanV1, PlaybackTimelineMapV1,
    StreamId,
};

use crate::probe::ProbeInventory;

pub fn select_default_stream(
    streams: &[MediaStreamV1],
    kind: MediaStreamKindV1,
    language_priority: &[String],
) -> Option<StreamId> {
    let mut candidates: Vec<&MediaStreamV1> = streams
        .iter()
        .filter(|stream| stream.kind == kind)
        .collect();
    candidates.sort_by_key(|stream| {
        let language_rank = stream
            .language
            .as_ref()
            .and_then(|language| language_priority.iter().position(|item| item == language))
            .unwrap_or(usize::MAX);
        (!stream.is_default, language_rank, stream.index)
    });
    candidates.first().map(|stream| stream.stream_id.clone())
}

#[must_use]
pub fn plan_playback(
    inventory: &ProbeInventory,
    report: &PlaybackCapabilityReportV1,
    decode: DecodeTestOutcomeV1,
    selected_video_stream_id: Option<&StreamId>,
    selected_audio_stream_id: Option<&StreamId>,
) -> PlaybackPlanV1 {
    let video = selected_video_stream_id
        .and_then(|selected| {
            inventory
                .streams
                .iter()
                .find(|stream| &stream.stream_id == selected)
        })
        .or_else(|| {
            inventory
                .streams
                .iter()
                .find(|stream| stream.kind == MediaStreamKindV1::Video)
        });
    let audio = selected_audio_stream_id
        .and_then(|selected| {
            inventory
                .streams
                .iter()
                .find(|stream| &stream.stream_id == selected)
        })
        .or_else(|| {
            inventory
                .streams
                .iter()
                .find(|stream| stream.kind == MediaStreamKindV1::Audio)
        });
    let stream_ids = video
        .into_iter()
        .chain(audio)
        .map(|stream| stream.stream_id.clone())
        .collect::<Vec<_>>();
    let video_codec = video.map(|stream| stream.codec.as_str());
    let audio_codec = audio.map(|stream| stream.codec.as_str());
    let exact_capable = candidate_supported(report, &inventory.container, video_codec, audio_codec);
    let target_video_capable = candidate_supported(report, "mp4", video_codec, Some("aac"));
    let timeline_map = PlaybackTimelineMapV1 {
        source_origin_us: inventory.source_origin_us,
        playback_origin_us: 0,
        source_offset_us: inventory.source_origin_us,
        source_duration_us: inventory.duration_us,
        playback_duration_us: inventory.duration_us,
    };

    let (kind, state, cost, reason) = if exact_capable && decode != DecodeTestOutcomeV1::Failed {
        (
            PlaybackPlanKindV1::Direct,
            if decode == DecodeTestOutcomeV1::FirstFramePresented {
                JobStateV1::Ready
            } else {
                JobStateV1::Probing
            },
            EstimatedCostV1::Negligible,
            "WEBVIEW_CAPABILITY_CONFIRMED",
        )
    } else if exact_capable && decode == DecodeTestOutcomeV1::Failed {
        (
            PlaybackPlanKindV1::TranscodeVideo,
            JobStateV1::AwaitingApproval,
            EstimatedCostV1::High,
            "ACTUAL_DECODE_FAILED",
        )
    } else if target_video_capable && matches!(audio_codec, None | Some("aac" | "mp3")) {
        (
            PlaybackPlanKindV1::Remux,
            JobStateV1::Queued,
            EstimatedCostV1::Low,
            "CONTAINER_REQUIRES_REMUX",
        )
    } else if target_video_capable {
        (
            PlaybackPlanKindV1::ConvertAudio,
            JobStateV1::Queued,
            EstimatedCostV1::Low,
            "AUDIO_CODEC_REQUIRES_CONVERSION",
        )
    } else if video.is_some() {
        (
            PlaybackPlanKindV1::TranscodeVideo,
            JobStateV1::AwaitingApproval,
            EstimatedCostV1::High,
            "VIDEO_CODEC_REQUIRES_TRANSCODE",
        )
    } else {
        (
            PlaybackPlanKindV1::Unsupported,
            JobStateV1::Failed,
            EstimatedCostV1::High,
            "NO_VIDEO_STREAM",
        )
    };
    let output_container = match kind {
        PlaybackPlanKindV1::Direct => Some(inventory.container.clone()),
        PlaybackPlanKindV1::Unsupported => None,
        _ => Some("mp4".into()),
    };
    let output_codecs = match kind {
        PlaybackPlanKindV1::ConvertAudio => {
            vec![video_codec.unwrap_or("copy").into(), "aac".into()]
        }
        PlaybackPlanKindV1::TranscodeVideo => vec!["h264".into(), "aac".into()],
        _ => video_codec
            .into_iter()
            .chain(audio_codec)
            .map(str::to_owned)
            .collect(),
    };
    PlaybackPlanV1 {
        kind,
        state,
        source_stream_ids: stream_ids,
        output_container,
        output_codecs,
        timeline_map,
        estimated_cost: cost,
        cache_key: None,
        progress: None,
        reason_codes: vec![reason.into()],
    }
}

fn candidate_supported(
    report: &PlaybackCapabilityReportV1,
    container: &str,
    video: Option<&str>,
    audio: Option<&str>,
) -> bool {
    report.candidates.iter().any(|candidate| {
        candidate.container == container
            && candidate.video_codec.as_deref() == video
            && candidate.audio_codec.as_deref() == audio
            && (matches!(candidate.can_play.as_str(), "probably" | "maybe")
                || candidate.media_capabilities_supported == Some(true))
    })
}

#[cfg(test)]
mod tests {
    use contracts::{CapabilityCandidateV1, DimensionsV1, SubtitleKindV1};

    use super::*;

    fn inventory(container: &str, video: &str, audio: &str) -> ProbeInventory {
        ProbeInventory {
            container: container.into(),
            container_start_us: 0,
            source_origin_us: 0,
            duration_us: 10_000_000,
            dimensions: Some(DimensionsV1 {
                width: 1280,
                height: 720,
            }),
            warnings: Vec::new(),
            streams: vec![
                MediaStreamV1 {
                    stream_id: StreamId::new("v0"),
                    index: 0,
                    kind: MediaStreamKindV1::Video,
                    codec: video.into(),
                    codec_profile: None,
                    language: None,
                    title: None,
                    is_default: true,
                    is_forced: false,
                    channels: None,
                    sample_rate: None,
                    width: Some(1280),
                    height: Some(720),
                    pixel_format: Some("yuv420p".into()),
                    frame_rate: Some(24.0),
                    subtitle_kind: None,
                },
                MediaStreamV1 {
                    stream_id: StreamId::new("a1"),
                    index: 1,
                    kind: MediaStreamKindV1::Audio,
                    codec: audio.into(),
                    codec_profile: None,
                    language: Some("jpn".into()),
                    title: None,
                    is_default: true,
                    is_forced: false,
                    channels: Some(2),
                    sample_rate: Some(48000),
                    width: None,
                    height: None,
                    pixel_format: None,
                    frame_rate: None,
                    subtitle_kind: Some(SubtitleKindV1::Text),
                },
            ],
        }
    }

    fn report(candidates: Vec<(&str, &str, &str)>) -> PlaybackCapabilityReportV1 {
        PlaybackCapabilityReportV1 {
            revision: 1,
            tested_webview: "WebView2-test".into(),
            candidates: candidates
                .into_iter()
                .map(|(container, video, audio)| CapabilityCandidateV1 {
                    container: container.into(),
                    video_codec: Some(video.into()),
                    audio_codec: Some(audio.into()),
                    can_play: "probably".into(),
                    media_capabilities_supported: Some(true),
                    media_capabilities_smooth: Some(true),
                })
                .collect(),
        }
    }

    #[test]
    fn chooses_cheapest_supported_path() {
        let capabilities = report(vec![("mp4", "h264", "aac")]);
        assert_eq!(
            plan_playback(
                &inventory("mp4", "h264", "aac"),
                &capabilities,
                DecodeTestOutcomeV1::NotTested,
                None,
                None,
            )
            .kind,
            PlaybackPlanKindV1::Direct
        );
        assert_eq!(
            plan_playback(
                &inventory("matroska", "h264", "aac"),
                &capabilities,
                DecodeTestOutcomeV1::NotTested,
                None,
                None,
            )
            .kind,
            PlaybackPlanKindV1::Remux
        );
        assert_eq!(
            plan_playback(
                &inventory("matroska", "h264", "flac"),
                &capabilities,
                DecodeTestOutcomeV1::NotTested,
                None,
                None,
            )
            .kind,
            PlaybackPlanKindV1::ConvertAudio
        );
        assert_eq!(
            plan_playback(
                &inventory("matroska", "hevc", "aac"),
                &capabilities,
                DecodeTestOutcomeV1::NotTested,
                None,
                None,
            )
            .kind,
            PlaybackPlanKindV1::TranscodeVideo
        );
        let actual_decode_failure = plan_playback(
            &inventory("mp4", "h264", "aac"),
            &capabilities,
            DecodeTestOutcomeV1::Failed,
            None,
            None,
        );
        assert_eq!(
            actual_decode_failure.kind,
            PlaybackPlanKindV1::TranscodeVideo
        );
        assert_eq!(actual_decode_failure.state, JobStateV1::AwaitingApproval);
    }
}
