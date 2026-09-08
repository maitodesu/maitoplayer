use std::{path::Path, path::PathBuf, sync::Arc, time::Duration};

use contracts::{
    AppErrorV1, DimensionsV1, MediaStreamKindV1, MediaStreamV1, StreamId, SubtitleKindV1,
    TimestampUs, error_codes,
};
use ports::{MediaToolPort, ToolRequest};
use serde::Deserialize;

const MAX_PROBE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ProbeInventory {
    pub container: String,
    pub container_start_us: TimestampUs,
    pub source_origin_us: TimestampUs,
    pub duration_us: TimestampUs,
    pub dimensions: Option<DimensionsV1>,
    pub streams: Vec<MediaStreamV1>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AssetProbe {
    pub duration_us: Option<TimestampUs>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub codec: String,
    pub kind: Option<MediaStreamKindV1>,
}

#[derive(Clone)]
pub struct ProbeService {
    ffprobe_path: PathBuf,
    runner: Arc<dyn MediaToolPort>,
}

impl std::fmt::Debug for ProbeService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProbeService")
            .field("ffprobe_path", &self.ffprobe_path)
            .finish_non_exhaustive()
    }
}

impl ProbeService {
    #[must_use]
    pub fn new(ffprobe_path: PathBuf, runner: Arc<dyn MediaToolPort>) -> Self {
        Self {
            ffprobe_path,
            runner,
        }
    }

    pub fn probe(&self, path: &Path, fingerprint: &str) -> Result<ProbeInventory, AppErrorV1> {
        let output = self.run_probe(path)?;
        parse_probe_json(&output, fingerprint)
    }

    pub(crate) fn probe_asset(&self, path: &Path) -> Result<AssetProbe, AppErrorV1> {
        let output = self.run_probe(path)?;
        parse_asset_probe(&output)
    }

    fn run_probe(&self, path: &Path) -> Result<Vec<u8>, AppErrorV1> {
        let output = self.runner.run(ToolRequest {
            executable: self.ffprobe_path.clone(),
            args: vec![
                "-v".into(),
                "error".into(),
                "-show_format".into(),
                "-show_streams".into(),
                "-of".into(),
                "json".into(),
                path.as_os_str().to_string_lossy().into_owned(),
            ],
            timeout: Duration::from_secs(15),
            max_stdout_bytes: MAX_PROBE_BYTES,
            max_stderr_bytes: 128 * 1024,
        })?;
        if output.status_code != Some(0) {
            return Err(AppErrorV1::new(
                error_codes::MEDIA_PROBE_FAILED,
                "The selected file could not be inspected as media.",
                false,
            ));
        }
        Ok(output.stdout)
    }
}

fn parse_asset_probe(bytes: &[u8]) -> Result<AssetProbe, AppErrorV1> {
    if bytes.len() > MAX_PROBE_BYTES {
        return Err(probe_shape_error("Probe output exceeded its size limit."));
    }
    let raw: RawProbe = serde_json::from_slice(bytes)
        .map_err(|error| probe_shape_error(&format!("Invalid asset probe JSON: {error}")))?;
    let stream = raw
        .streams
        .into_iter()
        .find(|stream| normalize_kind(stream.codec_type.as_deref()).is_some())
        .ok_or_else(|| probe_shape_error("Extracted asset had no media stream."))?;
    let duration_us = raw
        .format
        .and_then(|format| format.duration)
        .map(|duration| parse_seconds_us(Some(&duration)))
        .transpose()?;
    Ok(AssetProbe {
        duration_us,
        width: stream.width,
        height: stream.height,
        codec: stream.codec_name.unwrap_or_else(|| "unknown".into()),
        kind: normalize_kind(stream.codec_type.as_deref()),
    })
}

#[derive(Debug, Deserialize)]
struct RawProbe {
    #[serde(default)]
    streams: Vec<RawStream>,
    format: Option<RawFormat>,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    #[serde(default)]
    format_name: String,
    duration: Option<String>,
    start_time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawDisposition {
    #[serde(default)]
    default: i32,
    #[serde(default)]
    forced: i32,
}

#[derive(Debug, Default, Deserialize)]
struct RawTags {
    language: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawStream {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    r_frame_rate: Option<String>,
    channels: Option<u16>,
    sample_rate: Option<String>,
    start_time: Option<String>,
    #[serde(default)]
    disposition: RawDisposition,
    #[serde(default)]
    tags: RawTags,
}

pub fn parse_probe_json(bytes: &[u8], fingerprint: &str) -> Result<ProbeInventory, AppErrorV1> {
    if bytes.len() > MAX_PROBE_BYTES {
        return Err(probe_shape_error("Probe output exceeded its size limit."));
    }
    let raw: RawProbe = serde_json::from_slice(bytes)
        .map_err(|error| probe_shape_error(&format!("Invalid probe JSON: {error}")))?;
    let format = raw
        .format
        .ok_or_else(|| probe_shape_error("Probe output omitted format metadata."))?;
    let duration_us = parse_seconds_us(format.duration.as_deref())?;
    if duration_us <= 0 {
        return Err(probe_shape_error("Media duration is missing or invalid."));
    }
    let container_start_us = format
        .start_time
        .as_deref()
        .map(parse_timestamp_us)
        .transpose()?
        .unwrap_or(0);
    if container_start_us.abs() > 100_000 {
        return Err(timeline_error(
            "The container begins at a non-zero timestamp that cannot be represented safely.",
        ));
    }
    if format
        .format_name
        .split(',')
        .any(|name| matches!(name, "mpegts" | "hls"))
    {
        return Err(timeline_error(
            "Transport-stream timestamps may be discontinuous and are not accepted by the constant-offset timeline model.",
        ));
    }
    let mut warnings = Vec::new();
    if container_start_us != 0 {
        warnings.push(format!(
            "Normalized container timestamp origin {container_start_us}us to the canonical zero-based timeline."
        ));
    }
    let mut streams = Vec::new();
    for stream in raw.streams {
        let Some(kind) = normalize_kind(stream.codec_type.as_deref()) else {
            warnings.push(format!(
                "Ignored unsupported stream index {}.",
                stream.index
            ));
            continue;
        };
        if matches!(kind, MediaStreamKindV1::Video | MediaStreamKindV1::Audio)
            && let Some(start_time) = stream.start_time.as_deref()
        {
            let start_us = parse_timestamp_us(start_time)?;
            if start_us.abs_diff(container_start_us) > 100_000 {
                return Err(timeline_error(&format!(
                    "Stream {} begins too far from the canonical source origin.",
                    stream.index
                )));
            }
        }
        let codec = stream.codec_name.unwrap_or_else(|| "unknown".into());
        let subtitle_kind =
            (kind == MediaStreamKindV1::Subtitle).then(|| classify_subtitle(&codec));
        streams.push(MediaStreamV1 {
            stream_id: StreamId::new(format!(
                "{}:{}",
                &fingerprint[..fingerprint.len().min(16)],
                stream.index
            )),
            index: stream.index,
            kind,
            codec,
            codec_profile: stream.profile,
            language: stream.tags.language.map(|value| value.to_lowercase()),
            title: stream.tags.title,
            is_default: stream.disposition.default != 0,
            is_forced: stream.disposition.forced != 0,
            channels: stream.channels,
            sample_rate: stream.sample_rate.and_then(|value| value.parse().ok()),
            width: stream.width,
            height: stream.height,
            pixel_format: stream.pix_fmt,
            frame_rate: stream.r_frame_rate.as_deref().and_then(parse_rational),
            subtitle_kind,
        });
    }
    if !streams
        .iter()
        .any(|stream| stream.kind == MediaStreamKindV1::Video)
    {
        return Err(AppErrorV1::new(
            error_codes::MEDIA_UNSUPPORTED,
            "The selected file has no usable video stream.",
            false,
        ));
    }
    let dimensions = streams
        .iter()
        .find(|stream| stream.kind == MediaStreamKindV1::Video)
        .and_then(|stream| {
            Some(DimensionsV1 {
                width: stream.width?,
                height: stream.height?,
            })
        });
    Ok(ProbeInventory {
        container: normalize_container(&format.format_name),
        container_start_us,
        source_origin_us: 0,
        duration_us,
        dimensions,
        streams,
        warnings,
    })
}

fn normalize_kind(value: Option<&str>) -> Option<MediaStreamKindV1> {
    match value {
        Some("video") => Some(MediaStreamKindV1::Video),
        Some("audio") => Some(MediaStreamKindV1::Audio),
        Some("subtitle") => Some(MediaStreamKindV1::Subtitle),
        _ => None,
    }
}

fn normalize_container(value: &str) -> String {
    let names: Vec<_> = value.split(',').collect();
    if names
        .iter()
        .any(|name| *name == "matroska" || *name == "webm")
    {
        if names.contains(&"webm") && !names.contains(&"matroska") {
            "webm".into()
        } else {
            "matroska".into()
        }
    } else if names
        .iter()
        .any(|name| matches!(*name, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2"))
    {
        "mp4".into()
    } else {
        names.first().copied().unwrap_or("unknown").to_lowercase()
    }
}

fn classify_subtitle(codec: &str) -> SubtitleKindV1 {
    if matches!(
        codec,
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text"
    ) {
        SubtitleKindV1::Text
    } else {
        SubtitleKindV1::Image
    }
}

fn parse_seconds_us(value: Option<&str>) -> Result<i64, AppErrorV1> {
    let seconds: f64 = value
        .ok_or_else(|| probe_shape_error("Duration missing."))?
        .parse()
        .map_err(|_| probe_shape_error("Duration is not numeric."))?;
    if !seconds.is_finite() || seconds <= 0.0 || seconds > (i64::MAX as f64 / 1_000_000.0) {
        return Err(probe_shape_error("Duration is outside supported bounds."));
    }
    Ok((seconds * 1_000_000.0).round() as i64)
}

fn parse_timestamp_us(value: &str) -> Result<i64, AppErrorV1> {
    let seconds: f64 = value
        .parse()
        .map_err(|_| probe_shape_error("Timestamp is not numeric."))?;
    if !seconds.is_finite() || seconds.abs() > (i64::MAX as f64 / 1_000_000.0) {
        return Err(probe_shape_error("Timestamp is outside supported bounds."));
    }
    Ok((seconds * 1_000_000.0).round() as i64)
}

fn parse_rational(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator: f64 = numerator.parse().ok()?;
    let denominator: f64 = denominator.parse().ok()?;
    (denominator != 0.0).then_some(numerator / denominator)
}

fn probe_shape_error(detail: &str) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_PROBE_FAILED,
        "Media inspection returned invalid or excessive metadata.",
        false,
    )
    .with_diagnostics(detail)
}

fn timeline_error(detail: &str) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_UNSUPPORTED,
        "This media has a timestamp layout that cannot be normalized without risking subtitle or card drift.",
        false,
    )
    .with_diagnostics(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_probe_inventory_and_subtitle_kind() -> Result<(), AppErrorV1> {
        let fixture = br#"{
          "streams": [
            {"index":0,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"r_frame_rate":"24000/1001","disposition":{"default":1}},
            {"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000","tags":{"language":"JPN"}},
            {"index":2,"codec_type":"subtitle","codec_name":"hdmv_pgs_subtitle"}
          ],
          "format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"12.500"}
        }"#;
        let inventory = parse_probe_json(fixture, "abcdef0123456789")?;
        assert_eq!(inventory.container, "mp4");
        assert_eq!(inventory.container_start_us, 0);
        assert_eq!(inventory.source_origin_us, 0);
        assert_eq!(inventory.duration_us, 12_500_000);
        assert_eq!(inventory.streams[1].language.as_deref(), Some("jpn"));
        assert_eq!(
            inventory.streams[2].subtitle_kind,
            Some(SubtitleKindV1::Image)
        );
        Ok(())
    }

    #[test]
    fn rejects_non_zero_or_discontinuous_source_timelines() {
        let non_zero = br#"{
          "streams": [{"index":0,"codec_type":"video","codec_name":"h264","start_time":"5.000000"}],
          "format":{"format_name":"matroska","start_time":"5.000000","duration":"12.500"}
        }"#;
        assert!(matches!(
            parse_probe_json(non_zero, "abcdef0123456789"),
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::MEDIA_UNSUPPORTED
        ));
        let transport = br#"{
          "streams": [{"index":0,"codec_type":"video","codec_name":"h264","start_time":"0.000000"}],
          "format":{"format_name":"mpegts","start_time":"0.000000","duration":"12.500"}
        }"#;
        assert!(matches!(
            parse_probe_json(transport, "abcdef0123456789"),
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::MEDIA_UNSUPPORTED
        ));
    }
}
