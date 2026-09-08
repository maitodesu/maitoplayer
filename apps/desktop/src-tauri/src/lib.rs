mod commands;
mod mining;
mod playlist;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anki_connect::{CardPublisher, note::AnkiProfile, transport::HttpAnkiTransport};
use app_core::AppCore;
use contracts::{
    AppErrorV1, AppHealthV1, CardProfileSettingsV1, DependencyHealthV1, DictionaryEntrySummaryV1,
    TokenV1, UserSettingsV1, error_codes,
};
use dictionary::{EnrichedDictionary, LexicalMetadata, SqliteDictionary};
use japanese_nlp::LinderaTokenizer;
use media_engine::{
    MediaEngine,
    conversion::CachePolicy,
    toolchain::{discover_executable, tool_health},
};
use mining_assets::{AssetStore, ClipPolicy, MiningAssetService};
use parking_lot::RwLock;
use ports::{DictionaryPort, DraftRepositoryPort, PublishHistoryPort};
use storage::Storage;
use tauri::{
    Emitter, Manager,
    http::{Response, StatusCode, header},
};

use crate::mining::{MiningCoordinator, RecoveryReport};
use crate::playlist::PlaylistManager;

pub struct RuntimeState {
    core: Arc<AppCore>,
    media: Arc<MediaEngine>,
    mining: RwLock<Arc<MiningCoordinator>>,
    storage: Arc<Storage>,
    asset_store: Arc<AssetStore>,
    settings: RwLock<UserSettingsV1>,
    data_dir: PathBuf,
    base_health: AppHealthV1,
    recovery_report: Arc<RwLock<RecoveryReport>>,
    playlist: Arc<PlaylistManager>,
}

const SETTINGS_KEY: &str = "user.preferences";
const FFMPEG_PATH_KEY: &str = "tools.ffmpeg_path";
const FFPROBE_PATH_KEY: &str = "tools.ffprobe_path";
const DICTIONARY_PATH_KEY: &str = "dictionary.path";
const DEPENDENCY_OVERRIDE_ENV: &str = "MIGAKU_ENABLE_DEPENDENCY_OVERRIDES";
const BUNDLED_FFMPEG_PATH: &str = "bin/ffmpeg.exe";
const BUNDLED_FFPROBE_PATH: &str = "bin/ffprobe.exe";
const BUNDLED_DICTIONARY_PATH: &str = "data/jmdict.sqlite";
const BUNDLED_LEXICAL_METADATA_PATH: &str = "data/lexical-metadata.sqlite";

struct UnavailableDictionary;

impl DictionaryPort for UnavailableDictionary {
    fn lookup(&self, _token: &TokenV1) -> Result<Vec<DictionaryEntrySummaryV1>, AppErrorV1> {
        Err(AppErrorV1::new(
            error_codes::DICTIONARY_UNAVAILABLE,
            "The bundled JMdict database is unavailable. Repair or reinstall the application.",
            true,
        ))
    }

    fn version(&self) -> &str {
        "unavailable"
    }
}

pub fn run() {
    let application = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event
                && let Some(state) = window.app_handle().try_state::<RuntimeState>()
            {
                state.media.cancel_all();
            }
        })
        .register_uri_scheme_protocol("migaku-media", |context, request| {
            let Some(state) = context.app_handle().try_state::<RuntimeState>() else {
                return protocol_error(StatusCode::SERVICE_UNAVAILABLE);
            };
            let Some(session_value) = request
                .uri()
                .path()
                .trim_matches('/')
                .strip_prefix("session/")
            else {
                return protocol_error(StatusCode::NOT_FOUND);
            };
            if session_value.is_empty() || session_value.len() > 128 {
                return protocol_error(StatusCode::BAD_REQUEST);
            }
            let session_id = contracts::MediaSessionId::new(session_value);
            let total_size = match state.media.sessions().playback_size(&session_id) {
                Ok(size) => size,
                Err(_) => return protocol_error(StatusCode::NOT_FOUND),
            };
            let range_header = match request.headers().get(header::RANGE) {
                None => None,
                Some(value) => match value.to_str() {
                    Ok(value) => Some(value),
                    Err(_) => return protocol_range_error(total_size),
                },
            };
            let (start, end, is_range_request) =
                match resolve_protocol_range(range_header, total_size) {
                    Some(range) => range,
                    None => return protocol_range_error(total_size),
                };
            match state.media.sessions().read_range(&session_id, start, end) {
                Ok(slice) => {
                    let status = if is_range_request {
                        StatusCode::PARTIAL_CONTENT
                    } else {
                        StatusCode::OK
                    };
                    let builder = Response::builder()
                        .status(status)
                        .header(header::CONTENT_TYPE, slice.content_type)
                        .header(header::ACCEPT_RANGES, "bytes")
                        .header(header::CONTENT_LENGTH, slice.bytes.len().to_string())
                        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
                    let builder = if is_range_request {
                        builder.header(
                            header::CONTENT_RANGE,
                            format!(
                                "bytes {}-{}/{}",
                                slice.start, slice.end_inclusive, slice.total_size
                            ),
                        )
                    } else {
                        builder
                    };
                    builder
                        .body(slice.bytes)
                        .unwrap_or_else(|_| protocol_error(StatusCode::INTERNAL_SERVER_ERROR))
                }
                Err(error) if error.code == error_codes::INVALID_REQUEST => {
                    protocol_range_error(total_size)
                }
                Err(_) => protocol_error(StatusCode::NOT_FOUND),
            }
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let cache_dir = app.path().app_cache_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            std::fs::create_dir_all(&cache_dir)?;

            let storage = Arc::new(Storage::open(&data_dir.join("user.sqlite"))?);
            let settings = storage
                .get_setting::<UserSettingsV1>(SETTINGS_KEY)?
                .map(|(_, value)| value)
                .unwrap_or_else(default_user_settings);
            validate_user_settings(&settings)?;
            let resource_dir = app.path().resource_dir()?;
            let dependency_overrides_enabled = dependency_overrides_enabled();
            let configured_ffmpeg =
                selected_tool_path(storage.as_ref(), FFMPEG_PATH_KEY, "MIGAKU_FFMPEG_PATH")?;
            let configured_ffprobe =
                selected_tool_path(storage.as_ref(), FFPROBE_PATH_KEY, "MIGAKU_FFPROBE_PATH")?;
            let ffmpeg = resolve_media_tool(
                &resource_dir.join(BUNDLED_FFMPEG_PATH),
                configured_ffmpeg.as_deref(),
                "ffmpeg",
                dependency_overrides_enabled,
            );
            let ffprobe = resolve_media_tool(
                &resource_dir.join(BUNDLED_FFPROBE_PATH),
                configured_ffprobe.as_deref(),
                "ffprobe",
                dependency_overrides_enabled,
            );
            let media = Arc::new(MediaEngine::new_with_cache_policy(
                ffmpeg.clone(),
                ffprobe.clone(),
                cache_dir.join("playback"),
                cache_policy(&settings),
            ));
            media.set_audio_language_priority(settings.preferred_audio_languages.clone())?;
            let tokenizer = Arc::new(LinderaTokenizer::embedded_ipadic()?);
            let configured_dictionary = selected_tool_path(
                storage.as_ref(),
                DICTIONARY_PATH_KEY,
                "MIGAKU_DICTIONARY_PATH",
            )?;
            let dictionary_path = resolve_data_file(
                &resource_dir.join(BUNDLED_DICTIONARY_PATH),
                configured_dictionary.as_deref(),
                &data_dir.join("dictionary.sqlite"),
                dependency_overrides_enabled,
            );
            let (base_dictionary, dictionary_health):
                (Arc<dyn DictionaryPort>, DependencyHealthV1) =
                match SqliteDictionary::open(&dictionary_path) {
                    Ok(dictionary) => {
                        let version = dictionary.version().to_owned();
                        (
                            Arc::new(dictionary),
                            DependencyHealthV1 {
                                component: "JMdict dictionary".into(),
                                available: true,
                                version: Some(version),
                                action: None,
                                error: None,
                            },
                        )
                    }
                    Err(error) => (
                        Arc::new(UnavailableDictionary),
                        DependencyHealthV1 {
                            component: "JMdict dictionary".into(),
                            available: false,
                            version: None,
                            action: Some("Repair or reinstall the application.".into()),
                            error: Some(error),
                        },
                    ),
                };
            let lexical_metadata_path = resolve_data_file(
                &resource_dir.join(BUNDLED_LEXICAL_METADATA_PATH),
                None,
                &data_dir.join("lexical-metadata.sqlite"),
                dependency_overrides_enabled,
            );
            let (lexical_metadata, lexical_metadata_health) =
                match LexicalMetadata::open(&lexical_metadata_path) {
                    Ok(metadata) => {
                        let version = metadata.version().to_owned();
                        (
                            Some(metadata),
                            DependencyHealthV1 {
                                component: "Pitch accent and JLPT estimates".into(),
                                available: true,
                                version: Some(version),
                                action: None,
                                error: None,
                            },
                        )
                    }
                    Err(error) => (
                        None,
                        DependencyHealthV1 {
                            component: "Pitch accent and JLPT estimates".into(),
                            available: false,
                            version: None,
                            action: Some(
                                "Definitions remain available. Repair or reinstall to restore learning metadata."
                                    .into(),
                            ),
                            error: Some(error),
                        },
                    ),
                };
            let dictionary: Arc<dyn DictionaryPort> = Arc::new(EnrichedDictionary::new(
                base_dictionary,
                lexical_metadata,
            ));
            let drafts: Arc<dyn DraftRepositoryPort> = storage.clone();
            let core = Arc::new(AppCore::new(
                media.clone(),
                tokenizer,
                dictionary,
                drafts.clone(),
            ));
            let asset_store = Arc::new(AssetStore::open(cache_dir.join("mining-assets"))?);
            let mining = build_mining_coordinator(
                core.clone(),
                media.clone(),
                storage.clone(),
                asset_store.clone(),
                &settings,
            );
            let base_health = AppHealthV1 {
                checks: vec![
                    tool_health(ffmpeg.is_file().then_some(ffmpeg.as_path()), "FFmpeg"),
                    tool_health(ffprobe.is_file().then_some(ffprobe.as_path()), "FFprobe"),
                    DependencyHealthV1 {
                        component: "Japanese tokenizer".into(),
                        available: true,
                        version: Some("lindera-6.0.0-ipadic".into()),
                        action: None,
                        error: None,
                    },
                    dictionary_health,
                    lexical_metadata_health,
                ],
            };
            let recovery_report = Arc::new(RwLock::new(RecoveryReport::default()));
            let playlist = Arc::new(PlaylistManager::load(storage.clone())?);
            app.manage(RuntimeState {
                core,
                media,
                mining: RwLock::new(mining.clone()),
                storage,
                asset_store,
                settings: RwLock::new(settings),
                data_dir,
                base_health: base_health.clone(),
                recovery_report: recovery_report.clone(),
                playlist,
            });
            std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
                *recovery_report.write() = mining.resume_pending();
            }));
            let _ = app.emit("app-health", base_health);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::choose_and_import_media,
            commands::choose_and_import_playlist_folder,
            commands::current_playlist,
            commands::rescan_playlist,
            commands::import_playlist_discoveries,
            commands::select_playlist_item,
            commands::reorder_playlist_item,
            commands::remove_playlist_item,
            commands::choose_playlist_subtitle_override,
            commands::clear_playlist_subtitle_override,
            commands::report_playback_capabilities,
            commands::report_decode_outcome,
            commands::prepare_playback,
            commands::cancel_conversion,
            commands::conversion_progress,
            commands::mining_progress,
            commands::cancel_mining,
            commands::select_audio_stream,
            commands::send_playback_checkpoint,
            commands::choose_subtitle,
            commands::choose_translation_subtitle,
            commands::discover_subtitles,
            commands::use_discovered_subtitle,
            commands::restore_subtitle_binding,
            commands::restore_translation_subtitle_binding,
            commands::use_embedded_subtitle,
            commands::use_embedded_translation_subtitle,
            commands::remove_translation_subtitle,
            commands::subtitle_window,
            commands::translation_window,
            commands::create_draft,
            commands::publish_card,
            commands::close_media_session,
            commands::remember_media,
            commands::recent_media,
            commands::reopen_recent_media,
            commands::revoke_recent_media,
            commands::settings,
            commands::save_settings,
            commands::choose_ffmpeg,
            commands::choose_ffprobe,
            commands::choose_dictionary,
            commands::health,
        ])
        .run(tauri::generate_context!());
    if let Err(error) = application {
        eprintln!("application startup failed: {error}");
    }
}

fn default_user_settings() -> UserSettingsV1 {
    UserSettingsV1 {
        preferred_audio_languages: vec!["jpn".into(), "ja".into(), "und".into()],
        playback_cache_max_bytes: 20 * 1024 * 1024 * 1024,
        playback_cache_max_age_days: 30,
        clip_leading_padding_us: 250_000,
        clip_trailing_padding_us: 500_000,
        anki_port: 8765,
        anki_timeout_ms: 5_000,
        card_profile: CardProfileSettingsV1 {
            profile_id: "default".into(),
            deck_name: "Default".into(),
            model_name: "Migaku".into(),
            field_mapping: BTreeMap::from([
                ("expression".into(), "Expression".into()),
                ("reading".into(), "Reading".into()),
                ("sentence".into(), "Sentence".into()),
                ("definition".into(), "Definition".into()),
                ("audio".into(), "Audio".into()),
                ("image".into(), "Image".into()),
                ("source".into(), "Source".into()),
                ("timestamp".into(), "Timestamp".into()),
                ("mining_id".into(), "MiningId".into()),
            ]),
            tags: vec!["migaku".into(), "immersion".into()],
        },
    }
}

fn anki_profile(settings: &CardProfileSettingsV1) -> AnkiProfile {
    AnkiProfile {
        profile_id: settings.profile_id.clone(),
        deck_name: settings.deck_name.clone(),
        model_name: settings.model_name.clone(),
        field_mapping: settings.field_mapping.clone(),
        tags: settings.tags.clone(),
    }
}

fn build_mining_coordinator(
    core: Arc<AppCore>,
    media: Arc<MediaEngine>,
    storage: Arc<Storage>,
    asset_store: Arc<AssetStore>,
    settings: &UserSettingsV1,
) -> Arc<MiningCoordinator> {
    let drafts: Arc<dyn DraftRepositoryPort> = storage.clone();
    let history: Arc<dyn PublishHistoryPort> = storage.clone();
    let publisher = CardPublisher::new(
        Arc::new(HttpAnkiTransport::loopback(
            settings.anki_port,
            Duration::from_millis(u64::from(settings.anki_timeout_ms)),
        )),
        drafts,
        history,
        [anki_profile(&settings.card_profile)],
    );
    let clip_policy = ClipPolicy {
        leading_padding_us: settings.clip_leading_padding_us,
        trailing_padding_us: settings.clip_trailing_padding_us,
        ..ClipPolicy::default()
    };
    let asset_service = MiningAssetService::new(media, asset_store.clone(), clip_policy.clone());
    Arc::new(MiningCoordinator::new(
        core,
        asset_service,
        asset_store,
        publisher,
        storage,
        clip_policy,
    ))
}

fn cache_policy(settings: &UserSettingsV1) -> CachePolicy {
    CachePolicy {
        max_bytes: settings.playback_cache_max_bytes,
        max_age: Duration::from_secs(u64::from(settings.playback_cache_max_age_days) * 86_400),
    }
}

fn selected_tool_path(
    storage: &Storage,
    key: &str,
    environment_key: &str,
) -> Result<Option<PathBuf>, AppErrorV1> {
    Ok(std::env::var_os(environment_key)
        .map(PathBuf::from)
        .or(storage.get_setting::<PathBuf>(key)?.map(|(_, path)| path)))
}

fn dependency_overrides_enabled() -> bool {
    cfg!(debug_assertions)
        || std::env::var(DEPENDENCY_OVERRIDE_ENV)
            .is_ok_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

fn preferred_existing_path(
    bundled: &std::path::Path,
    configured: Option<&std::path::Path>,
    allow_override: bool,
) -> Option<PathBuf> {
    if allow_override
        && let Some(configured) = configured
        && configured.is_file()
    {
        return Some(configured.to_path_buf());
    }
    bundled.is_file().then(|| bundled.to_path_buf())
}

fn resolve_media_tool(
    bundled: &std::path::Path,
    configured: Option<&std::path::Path>,
    name: &str,
    allow_override: bool,
) -> PathBuf {
    let discovered = allow_override
        .then(|| discover_executable(None, name))
        .flatten();
    resolve_media_tool_candidate(bundled, configured, discovered.as_deref(), allow_override)
}

fn resolve_media_tool_candidate(
    bundled: &std::path::Path,
    configured: Option<&std::path::Path>,
    discovered: Option<&std::path::Path>,
    allow_override: bool,
) -> PathBuf {
    if !allow_override {
        return bundled.to_path_buf();
    }
    preferred_existing_path(bundled, configured, true)
        .or_else(|| discovered.map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| bundled.to_path_buf())
}

fn resolve_data_file(
    bundled: &std::path::Path,
    configured: Option<&std::path::Path>,
    development_fallback: &std::path::Path,
    allow_override: bool,
) -> PathBuf {
    preferred_existing_path(bundled, configured, allow_override).unwrap_or_else(|| {
        if allow_override {
            configured.unwrap_or(development_fallback).to_path_buf()
        } else {
            bundled.to_path_buf()
        }
    })
}

fn validate_user_settings(settings: &UserSettingsV1) -> Result<(), AppErrorV1> {
    let languages_valid = !settings.preferred_audio_languages.is_empty()
        && settings.preferred_audio_languages.len() <= 16
        && settings.preferred_audio_languages.iter().all(|language| {
            !language.is_empty()
                && language.len() <= 16
                && language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    let allowed_fields = BTreeSet::from([
        "expression",
        "reading",
        "sentence",
        "definition",
        "audio",
        "image",
        "source",
        "timestamp",
        "mining_id",
    ]);
    let mapped_targets = settings
        .card_profile
        .field_mapping
        .values()
        .collect::<BTreeSet<_>>();
    let profile_valid = !settings.card_profile.profile_id.is_empty()
        && settings.card_profile.profile_id.len() <= 128
        && settings
            .card_profile
            .profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && !settings.card_profile.deck_name.trim().is_empty()
        && settings.card_profile.deck_name.len() <= 255
        && !settings.card_profile.model_name.trim().is_empty()
        && settings.card_profile.model_name.len() <= 255
        && !settings.card_profile.field_mapping.is_empty()
        && settings.card_profile.field_mapping.len() == mapped_targets.len()
        && settings
            .card_profile
            .field_mapping
            .iter()
            .all(|(logical, target)| {
                allowed_fields.contains(logical.as_str())
                    && !target.trim().is_empty()
                    && target.len() <= 255
            })
        && settings.card_profile.tags.len() <= 64
        && settings
            .card_profile
            .tags
            .iter()
            .all(|tag| !tag.is_empty() && tag.len() <= 128);
    if !languages_valid
        || !(1024 * 1024 * 1024..=200 * 1024 * 1024 * 1024)
            .contains(&settings.playback_cache_max_bytes)
        || !(1..=365).contains(&settings.playback_cache_max_age_days)
        || !(0..=5_000_000).contains(&settings.clip_leading_padding_us)
        || !(0..=5_000_000).contains(&settings.clip_trailing_padding_us)
        || settings.anki_port == 0
        || !(250..=30_000).contains(&settings.anki_timeout_ms)
        || !profile_valid
    {
        return Err(AppErrorV1::new(
            error_codes::INVALID_REQUEST,
            "One or more settings are outside their allowed range.",
            false,
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestedRange {
    Inclusive(u64, Option<u64>),
    Suffix(u64),
}

fn parse_range(value: &str) -> Option<RequestedRange> {
    let range = value.strip_prefix("bytes=")?;
    if range.contains(',') {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse().ok()?;
        return (suffix > 0).then_some(RequestedRange::Suffix(suffix));
    }
    let start = start.parse().ok()?;
    let end = if end.is_empty() {
        None
    } else {
        Some(end.parse().ok()?)
    };
    if end.is_some_and(|end| end < start) {
        return None;
    }
    Some(RequestedRange::Inclusive(start, end))
}

fn resolve_protocol_range(
    header: Option<&str>,
    total_size: u64,
) -> Option<(u64, Option<u64>, bool)> {
    match header.map(parse_range) {
        Some(Some(RequestedRange::Inclusive(start, end))) => Some((start, end, true)),
        Some(Some(RequestedRange::Suffix(length))) => Some((
            total_size.saturating_sub(length),
            Some(total_size.saturating_sub(1)),
            true,
        )),
        Some(None) => None,
        None if total_size <= media_engine::session::MAX_PROTOCOL_CHUNK_BYTES as u64 => {
            Some((0, None, false))
        }
        None => None,
    }
}

fn protocol_error(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn protocol_range_error(total_size: u64) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::CONTENT_RANGE, format!("bytes */{total_size}"))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, "0")
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_valid_but_duplicate_anki_targets_are_rejected() -> Result<(), AppErrorV1>
    {
        let mut settings = default_user_settings();
        assert!(validate_user_settings(&settings).is_ok());
        settings
            .card_profile
            .field_mapping
            .insert("reading".into(), "Expression".into());
        let Err(error) = validate_user_settings(&settings) else {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "Duplicate Anki targets were unexpectedly accepted.",
                false,
            ));
        };
        assert_eq!(error.code, error_codes::INVALID_REQUEST);
        let mut non_default = default_user_settings();
        non_default.card_profile.profile_id = "japanese-mining".into();
        assert!(validate_user_settings(&non_default).is_ok());
        Ok(())
    }

    #[test]
    fn scoped_protocol_rejects_suffix_and_reversed_ranges() {
        assert_eq!(
            parse_range("bytes=0-99"),
            Some(RequestedRange::Inclusive(0, Some(99)))
        );
        assert_eq!(
            parse_range("bytes=100-"),
            Some(RequestedRange::Inclusive(100, None))
        );
        assert_eq!(parse_range("bytes=-100"), Some(RequestedRange::Suffix(100)));
        assert_eq!(parse_range("bytes=-0"), None);
        assert_eq!(parse_range("bytes=100-99"), None);
        assert_eq!(parse_range("bytes=0-1,4-5"), None);
        assert_eq!(
            resolve_protocol_range(Some("bytes=-100"), 1_000),
            Some((900, Some(999), true))
        );
        assert!(resolve_protocol_range(Some("bytes=broken"), 1_000).is_none());
        assert!(
            resolve_protocol_range(
                None,
                media_engine::session::MAX_PROTOCOL_CHUNK_BYTES as u64 + 1,
            )
            .is_none()
        );
    }

    #[test]
    fn packaged_dependencies_win_unless_development_override_is_explicit()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let bundled = directory.path().join("bundled.exe");
        let configured = directory.path().join("configured.exe");
        std::fs::write(&bundled, b"bundled")?;
        std::fs::write(&configured, b"configured")?;

        assert_eq!(
            preferred_existing_path(&bundled, Some(&configured), false),
            Some(bundled.clone())
        );
        assert_eq!(
            preferred_existing_path(&bundled, Some(&configured), true),
            Some(configured)
        );
        Ok(())
    }

    #[test]
    fn missing_packaged_dictionary_does_not_silently_use_user_data_in_release()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let bundled = directory.path().join("missing-bundled.sqlite");
        let configured = directory.path().join("configured.sqlite");
        let fallback = directory.path().join("managed.sqlite");
        std::fs::write(&configured, b"configured")?;

        assert_eq!(
            resolve_data_file(&bundled, Some(&configured), &fallback, false),
            bundled
        );
        assert_eq!(
            resolve_data_file(
                &directory.path().join("missing.sqlite"),
                Some(&configured),
                &fallback,
                true
            ),
            configured
        );
        Ok(())
    }

    #[test]
    fn missing_packaged_media_tool_does_not_execute_discovered_path_in_release()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let bundled = directory.path().join("missing-bundled.exe");
        let configured = directory.path().join("configured.exe");
        let discovered = directory.path().join("path-injected.exe");
        std::fs::write(&configured, b"configured")?;
        std::fs::write(&discovered, b"path-injected")?;

        assert_eq!(
            resolve_media_tool_candidate(&bundled, Some(&configured), Some(&discovered), false,),
            bundled
        );
        assert_eq!(
            resolve_media_tool_candidate(
                &directory.path().join("missing-debug-bundle.exe"),
                None,
                Some(&discovered),
                true,
            ),
            discovered
        );
        Ok(())
    }
}
