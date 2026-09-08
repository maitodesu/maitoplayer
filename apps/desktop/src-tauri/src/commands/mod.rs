use app_core::{AppCore, DraftSelection};
use contracts::{
    AppErrorV1, AppHealthV1, CardDraftV1, CreateCardRequestV1, CreateCardResultV1,
    DecodeTestOutcomeV1, DependencyHealthV1, MediaSessionId, MediaSessionV1,
    PlaybackCapabilityReportV1, PlaybackCheckpointV1, RecentMediaV1, StreamId, SubtitleCandidateV1,
    SubtitleDiscoveryV1, SubtitleOriginV1, SubtitleSourceId, SubtitleSourceV1, SubtitleWindowV1,
    ToolSelectionResultV1, TranslationWindowV1, UserSettingsV1,
};
use dictionary::SqliteDictionary;
use media_engine::toolchain::inspect_tool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtitle_core::discovery::{choose as choose_discovered, discover_adjacent};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

use crate::playlist::{
    PlaylistManager, PlaylistScanModeV1, PlaylistScanResultV1, PlaylistSelectionV1,
    PlaylistSubtitleMutationV1, PlaylistV1, ResolvedPlaylistItem, ResolvedPlaylistSubtitle,
    SubtitleRoleV1,
};
use crate::{
    DICTIONARY_PATH_KEY, FFMPEG_PATH_KEY, FFPROBE_PATH_KEY, RuntimeState, build_mining_coordinator,
    cache_policy, validate_user_settings,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredSubtitleBinding {
    path: std::path::PathBuf,
    origin: SubtitleOriginV1,
    embedded_stream_id: Option<StreamId>,
    language: Option<String>,
}

#[tauri::command]
pub async fn choose_and_import_media(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<Option<MediaSessionV1>, AppErrorV1> {
    let selected = app
        .dialog()
        .file()
        .add_filter("Video", &["mp4", "m4v", "webm", "mkv", "mov", "avi"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|error| {
        AppErrorV1::new(
            contracts::error_codes::MEDIA_SCOPE_DENIED,
            "The selected file could not be authorized.",
            true,
        )
        .with_diagnostics(error.to_string())
    })?;
    let mut session = state.core.import_media(path)?;
    if let Some(recent) = state.storage.recent(&session.source_fingerprint)? {
        let (canonical_path, _, _) = state.media.source_identity_context(&session.session_id)?;
        if canonical_path == recent.path {
            session = state.media.mark_persistent(&session.session_id)?;
            state.core.sync_session(session.clone());
        }
    }
    match state.playlist.clear_current() {
        Ok(Some(playlist)) => {
            let _ = app.emit("playlist-changed", &playlist);
        }
        Ok(None) => {}
        Err(error) => {
            let _ = state.core.close_session(&session.session_id);
            return Err(error);
        }
    }
    let _ = app.emit("media-session-changed", &session);
    Ok(Some(session))
}

#[tauri::command]
pub async fn choose_and_import_playlist_folder(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    scan_mode: PlaylistScanModeV1,
) -> Result<Option<PlaylistScanResultV1>, AppErrorV1> {
    let selected = app.dialog().file().blocking_pick_folder();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(scope_error)?;
    let playlist = state.playlist.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || playlist.import_folder(&path, scan_mode))
            .await
            .map_err(playlist_worker_error)??;
    let _ = app.emit("playlist-changed", &result.playlist);
    Ok(Some(result))
}

#[tauri::command]
pub fn current_playlist(state: State<'_, RuntimeState>) -> Option<PlaylistV1> {
    state.playlist.current()
}

#[tauri::command]
pub async fn rescan_playlist(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    scan_mode: PlaylistScanModeV1,
) -> Result<PlaylistScanResultV1, AppErrorV1> {
    let previous = state.playlist.current();
    let playlist = state.playlist.clone();
    let result = tauri::async_runtime::spawn_blocking(move || playlist.rescan(scan_mode))
        .await
        .map_err(playlist_worker_error)??;
    if previous.as_ref().is_none_or(|previous| {
        previous.revision != result.playlist.revision
            || previous.scan_revision != result.playlist.scan_revision
    }) {
        let _ = app.emit("playlist-changed", &result.playlist);
    }
    Ok(result)
}

#[tauri::command]
pub async fn import_playlist_discoveries(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    item_ids: Vec<String>,
) -> Result<PlaylistV1, AppErrorV1> {
    let playlist = state.playlist.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || playlist.import_discoveries(&item_ids))
            .await
            .map_err(playlist_worker_error)??;
    let _ = app.emit("playlist-changed", &result);
    Ok(result)
}

#[tauri::command]
pub async fn select_playlist_item(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    item_id: String,
) -> Result<PlaylistSelectionV1, AppErrorV1> {
    let playlist = state.playlist.clone();
    let core = state.core.clone();
    let media = state.media.clone();
    let storage = state.storage.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let resolved = playlist.resolve_item(&item_id)?;
        let imported = core.import_media(resolved.path.clone())?;
        let session_id = imported.session_id.clone();
        let result: Result<PlaylistSelectionV1, AppErrorV1> = (|| {
            storage.consent_recent(
                &imported.source_fingerprint,
                &resolved.path,
                &imported.display_name,
                true,
            )?;
            let session = media.mark_persistent(&session_id)?;
            core.sync_session(session.clone());
            let japanese_subtitle = resolved
                .japanese_subtitle
                .as_ref()
                .map(|subtitle| {
                    register_playlist_subtitle(
                        core.as_ref(),
                        storage.as_ref(),
                        &session_id,
                        &session.source_fingerprint,
                        subtitle,
                        SubtitleRoleV1::Japanese,
                    )
                })
                .transpose()?;
            let translation_subtitle = resolved
                .translation_subtitle
                .as_ref()
                .map(|subtitle| {
                    register_playlist_subtitle(
                        core.as_ref(),
                        storage.as_ref(),
                        &session_id,
                        &session.source_fingerprint,
                        subtitle,
                        SubtitleRoleV1::Translation,
                    )
                })
                .transpose()?;
            let playlist = playlist.select_item(&item_id, &session_id)?;
            Ok(PlaylistSelectionV1 {
                playlist,
                session,
                japanese_subtitle,
                translation_subtitle,
            })
        })();
        if result.is_err() {
            let _ = core.close_session(&session_id);
        }
        result
    })
    .await
    .map_err(playlist_worker_error)??;
    let _ = app.emit("playlist-changed", &result.playlist);
    let _ = app.emit("media-session-changed", &result.session);
    Ok(result)
}

#[tauri::command]
pub async fn reorder_playlist_item(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    item_id: String,
    new_index: usize,
) -> Result<PlaylistV1, AppErrorV1> {
    let playlist = state.playlist.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || playlist.reorder_item(&item_id, new_index))
            .await
            .map_err(playlist_worker_error)??;
    let _ = app.emit("playlist-changed", &result);
    Ok(result)
}

#[tauri::command]
pub async fn remove_playlist_item(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    item_id: String,
) -> Result<PlaylistV1, AppErrorV1> {
    let playlist = state.playlist.clone();
    let result = tauri::async_runtime::spawn_blocking(move || playlist.remove_item(&item_id))
        .await
        .map_err(playlist_worker_error)??;
    let _ = app.emit("playlist-changed", &result);
    Ok(result)
}

#[tauri::command]
pub async fn choose_playlist_subtitle_override(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    item_id: String,
    role: SubtitleRoleV1,
    session_id: Option<MediaSessionId>,
) -> Result<Option<PlaylistSubtitleMutationV1>, AppErrorV1> {
    let filter_name = match role {
        SubtitleRoleV1::Japanese => "Japanese study subtitles",
        SubtitleRoleV1::Translation => "English translation subtitles",
    };
    let selected = app
        .dialog()
        .file()
        .add_filter(filter_name, &["srt", "ass", "ssa", "vtt"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected_path = selected.into_path().map_err(scope_error)?;
    let playlist = state.playlist.clone();
    let core = state.core.clone();
    let media = state.media.clone();
    let storage = state.storage.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let resolved = playlist.resolve_item(&item_id)?;
        let override_path = playlist.canonical_subtitle_override(&selected_path)?;
        let source = session_id
            .as_ref()
            .map(|session_id| {
                validate_active_playlist_session(
                    playlist.as_ref(),
                    media.as_ref(),
                    &resolved,
                    session_id,
                )?;
                let (_, fingerprint, _) = media.source_identity_context(session_id)?;
                register_playlist_subtitle(
                    core.as_ref(),
                    storage.as_ref(),
                    session_id,
                    &fingerprint,
                    &ResolvedPlaylistSubtitle {
                        path: override_path.clone(),
                        origin: SubtitleOriginV1::SelectedFile,
                    },
                    role,
                )
            })
            .transpose()?;
        let playlist = playlist.set_subtitle_override(&item_id, role, Some(override_path))?;
        Ok::<_, AppErrorV1>(PlaylistSubtitleMutationV1 { playlist, source })
    })
    .await
    .map_err(playlist_worker_error)??;
    let _ = app.emit("playlist-changed", &result.playlist);
    Ok(Some(result))
}

#[tauri::command]
pub async fn clear_playlist_subtitle_override(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    item_id: String,
    role: SubtitleRoleV1,
    session_id: Option<MediaSessionId>,
) -> Result<PlaylistSubtitleMutationV1, AppErrorV1> {
    let playlist = state.playlist.clone();
    let core = state.core.clone();
    let media = state.media.clone();
    let storage = state.storage.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let resolved = playlist.resolve_item(&item_id)?;
        let automatic = playlist.subtitle_path_after_clear(&item_id, role)?;
        let source = session_id
            .as_ref()
            .map(|session_id| {
                validate_active_playlist_session(
                    playlist.as_ref(),
                    media.as_ref(),
                    &resolved,
                    session_id,
                )?;
                let (_, fingerprint, _) = media.source_identity_context(session_id)?;
                match automatic.as_ref() {
                    Some(subtitle) => register_playlist_subtitle(
                        core.as_ref(),
                        storage.as_ref(),
                        session_id,
                        &fingerprint,
                        subtitle,
                        role,
                    )
                    .map(Some),
                    None => {
                        remove_playlist_subtitle_binding(storage.as_ref(), &fingerprint, role)?;
                        Ok(None)
                    }
                }
            })
            .transpose()?
            .flatten();
        let playlist = playlist.set_subtitle_override(&item_id, role, None)?;
        Ok::<_, AppErrorV1>(PlaylistSubtitleMutationV1 { playlist, source })
    })
    .await
    .map_err(playlist_worker_error)??;
    let _ = app.emit("playlist-changed", &result.playlist);
    Ok(result)
}

#[tauri::command]
pub fn report_playback_capabilities(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    report: PlaybackCapabilityReportV1,
) -> Result<MediaSessionV1, AppErrorV1> {
    let session = state.core.apply_capabilities(&session_id, report)?;
    let _ = app.emit("media-session-changed", &session);
    Ok(session)
}

#[tauri::command]
pub fn report_decode_outcome(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    outcome: DecodeTestOutcomeV1,
) -> Result<MediaSessionV1, AppErrorV1> {
    let session = state.core.report_decode_outcome(&session_id, outcome)?;
    let _ = app.emit("media-session-changed", &session);
    Ok(session)
}

#[tauri::command]
pub async fn prepare_playback(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    approved_video_transcode: bool,
) -> Result<MediaSessionV1, AppErrorV1> {
    let core = state.core.clone();
    let session = tauri::async_runtime::spawn_blocking(move || {
        core.prepare_playback(&session_id, approved_video_transcode)
    })
    .await
    .map_err(|error| {
        AppErrorV1::new(
            contracts::error_codes::CONVERSION_FAILED,
            "The playback conversion worker stopped unexpectedly.",
            true,
        )
        .with_diagnostics(error.to_string())
    })??;
    let _ = app.emit("media-session-changed", &session);
    Ok(session)
}

#[tauri::command]
pub fn cancel_conversion(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
) -> Result<bool, AppErrorV1> {
    state.core.cancel_conversion(&session_id)
}

#[tauri::command]
pub fn conversion_progress(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
) -> Option<f32> {
    state.media.conversion_progress(&session_id)
}

#[tauri::command]
pub fn mining_progress(state: State<'_, RuntimeState>, session_id: MediaSessionId) -> Option<f32> {
    state.media.mining_progress(&session_id)
}

#[tauri::command]
pub fn cancel_mining(state: State<'_, RuntimeState>, session_id: MediaSessionId) -> bool {
    let publish_cancelled = state.mining.read().cancel_session(&session_id);
    state.media.cancel_mining(&session_id) || publish_cancelled
}

#[tauri::command]
pub fn select_audio_stream(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    stream_id: StreamId,
) -> Result<MediaSessionV1, AppErrorV1> {
    let session = state.core.select_audio_stream(&session_id, &stream_id)?;
    let _ = app.emit("media-session-changed", &session);
    Ok(session)
}

#[tauri::command]
pub fn send_playback_checkpoint(
    state: State<'_, RuntimeState>,
    checkpoint: PlaybackCheckpointV1,
) -> Result<bool, AppErrorV1> {
    state.core.checkpoint(checkpoint)
}

#[tauri::command]
pub async fn choose_subtitle(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
) -> Result<Option<SubtitleSourceV1>, AppErrorV1> {
    let selected = app
        .dialog()
        .file()
        .add_filter("Text subtitles", &["srt", "ass", "ssa", "vtt"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|error| {
        AppErrorV1::new(
            contracts::error_codes::MEDIA_SCOPE_DENIED,
            "The selected subtitle file could not be authorized.",
            true,
        )
        .with_diagnostics(error.to_string())
    })?;
    register_and_bind(&state, &session_id, &path, SubtitleOriginV1::SelectedFile).map(Some)
}

#[tauri::command]
pub async fn choose_translation_subtitle(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
) -> Result<Option<SubtitleSourceV1>, AppErrorV1> {
    let selected = app
        .dialog()
        .file()
        .add_filter(
            "English translation subtitles",
            &["srt", "ass", "ssa", "vtt"],
        )
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|error| {
        AppErrorV1::new(
            contracts::error_codes::MEDIA_SCOPE_DENIED,
            "The selected translation subtitle file could not be authorized.",
            true,
        )
        .with_diagnostics(error.to_string())
    })?;
    register_translation_and_bind(&state, &session_id, &path, SubtitleOriginV1::SelectedFile)
        .map(Some)
}

#[tauri::command]
pub fn discover_subtitles(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
) -> Result<SubtitleDiscoveryV1, AppErrorV1> {
    let (media_path, fingerprint, _) = state.media.source_identity_context(&session_id)?;
    let candidates = discover_adjacent(&media_path)?;
    let priorities = state.settings.read().preferred_audio_languages.clone();
    let recommended_candidate_id = match choose_discovered(&candidates, &priorities) {
        Ok(Some(candidate)) => Some(subtitle_candidate_id(&fingerprint, &candidate.path)),
        Ok(None) => None,
        Err(error) if error.code == contracts::error_codes::SUBTITLE_AMBIGUOUS => None,
        Err(error) => return Err(error),
    };
    let candidates = candidates
        .into_iter()
        .map(|candidate| SubtitleCandidateV1 {
            candidate_id: subtitle_candidate_id(&fingerprint, &candidate.path),
            display_name: candidate
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Adjacent subtitles".into()),
            format: candidate.format,
            language: candidate.language,
        })
        .collect();
    Ok(SubtitleDiscoveryV1 {
        candidates,
        recommended_candidate_id,
    })
}

#[tauri::command]
pub fn use_discovered_subtitle(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    candidate_id: String,
) -> Result<SubtitleSourceV1, AppErrorV1> {
    validate_candidate_id(&candidate_id)?;
    let (media_path, fingerprint, _) = state.media.source_identity_context(&session_id)?;
    let candidate = discover_adjacent(&media_path)?
        .into_iter()
        .find(|candidate| subtitle_candidate_id(&fingerprint, &candidate.path) == candidate_id)
        .ok_or_else(|| {
            AppErrorV1::new(
                contracts::error_codes::STALE_REVISION,
                "The adjacent subtitle choices changed. Refresh them before selecting.",
                true,
            )
        })?;
    register_and_bind(
        &state,
        &session_id,
        &candidate.path,
        SubtitleOriginV1::AdjacentFile,
    )
}

#[tauri::command]
pub fn restore_subtitle_binding(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
) -> Result<Option<SubtitleSourceV1>, AppErrorV1> {
    let (_, fingerprint, _) = state.media.source_identity_context(&session_id)?;
    let Some((stored_version, binding)) = state
        .storage
        .subtitle_binding::<StoredSubtitleBinding>(&fingerprint)?
    else {
        return Ok(None);
    };
    match subtitle_core::source::load(&binding.path) {
        Ok(loaded) if loaded.source_version == stored_version => loaded,
        Ok(_) | Err(_) => return Ok(None),
    };
    let source = match (binding.origin, binding.embedded_stream_id) {
        (SubtitleOriginV1::EmbeddedTextTrack, Some(stream_id)) => state
            .core
            .register_embedded_subtitle(&session_id, &binding.path, stream_id, binding.language)?,
        (origin, _) => state
            .core
            .register_subtitle(&session_id, &binding.path, origin)?,
    };
    Ok(Some(source))
}

#[tauri::command]
pub fn restore_translation_subtitle_binding(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
) -> Result<Option<SubtitleSourceV1>, AppErrorV1> {
    let (_, fingerprint, _) = state.media.source_identity_context(&session_id)?;
    let Some((stored_version, binding)) = state
        .storage
        .translation_subtitle_binding::<StoredSubtitleBinding>(&fingerprint)?
    else {
        return Ok(None);
    };
    match subtitle_core::source::load(&binding.path) {
        Ok(loaded) if loaded.source_version == stored_version => loaded,
        Ok(_) | Err(_) => return Ok(None),
    };
    let source = match (binding.origin, binding.embedded_stream_id) {
        (SubtitleOriginV1::EmbeddedTextTrack, Some(stream_id)) => {
            state.core.register_embedded_translation_subtitle(
                &session_id,
                &binding.path,
                stream_id,
                binding.language,
            )?
        }
        (origin, _) => {
            state
                .core
                .register_translation_subtitle(&session_id, &binding.path, origin)?
        }
    };
    Ok(Some(source))
}

#[tauri::command]
pub fn remember_media(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    subtitle_source_id: Option<SubtitleSourceId>,
    translation_source_id: Option<SubtitleSourceId>,
) -> Result<MediaSessionV1, AppErrorV1> {
    let (path, fingerprint, display_name) = state.media.source_identity_context(&session_id)?;
    state
        .storage
        .consent_recent(&fingerprint, &path, &display_name, true)?;
    let session = state.media.mark_persistent(&session_id)?;
    state.core.sync_session(session.clone());
    if let Some(source_id) = subtitle_source_id {
        let (subtitle_path, subtitle_source) = state
            .core
            .interactive_subtitle_binding(&session_id, &source_id)?;
        store_subtitle_binding(
            state.storage.as_ref(),
            &fingerprint,
            &subtitle_path,
            &subtitle_source,
        )?;
    }
    if let Some(source_id) = translation_source_id {
        let (subtitle_path, subtitle_source) = state
            .core
            .translation_subtitle_binding(&session_id, &source_id)?;
        store_translation_subtitle_binding(
            state.storage.as_ref(),
            &fingerprint,
            &subtitle_path,
            &subtitle_source,
        )?;
    }
    Ok(session)
}

#[tauri::command]
pub fn recent_media(state: State<'_, RuntimeState>) -> Result<Vec<RecentMediaV1>, AppErrorV1> {
    state.storage.recent_media(20).map(|items| {
        items
            .into_iter()
            .map(|item| RecentMediaV1 {
                source_fingerprint: item.source_fingerprint,
                display_name: item.display_name,
            })
            .collect()
    })
}

#[tauri::command]
pub fn reopen_recent_media(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    source_fingerprint: String,
) -> Result<MediaSessionV1, AppErrorV1> {
    let recent = state
        .storage
        .recent(&source_fingerprint)?
        .ok_or_else(|| scope_error("The approved recent file is no longer registered."))?;
    let imported = state.core.import_media(recent.path.clone())?;
    if imported.source_fingerprint != source_fingerprint {
        let _ = state.core.close_session(&imported.session_id);
        return Err(scope_error(
            "The recent file changed since consent was granted. Locate and approve it again.",
        ));
    }
    state.storage.consent_recent(
        &source_fingerprint,
        &recent.path,
        &recent.display_name,
        true,
    )?;
    let session = state.media.mark_persistent(&imported.session_id)?;
    state.core.sync_session(session.clone());
    let _ = app.emit("media-session-changed", &session);
    Ok(session)
}

#[tauri::command]
pub fn revoke_recent_media(
    state: State<'_, RuntimeState>,
    source_fingerprint: String,
) -> Result<bool, AppErrorV1> {
    state.storage.revoke_recent(&source_fingerprint)
}

#[tauri::command]
pub async fn use_embedded_subtitle(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    stream_id: StreamId,
) -> Result<SubtitleSourceV1, AppErrorV1> {
    let media = state.media.clone();
    let core = state.core.clone();
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let extracted = media.extract_embedded_subtitle(&session_id, &stream_id)?;
        let source = core.register_embedded_subtitle(
            &session_id,
            &extracted.path,
            extracted.stream_id,
            extracted.language,
        )?;
        if core.session(&session_id)?.source_scope_persistence
            == contracts::SourceScopePersistenceV1::UserApprovedPersistent
        {
            let (_, fingerprint, _) = media.source_identity_context(&session_id)?;
            store_subtitle_binding(storage.as_ref(), &fingerprint, &extracted.path, &source)?;
        }
        Ok(source)
    })
    .await
    .map_err(|error| {
        AppErrorV1::new(
            contracts::error_codes::CONVERSION_FAILED,
            "The subtitle extraction worker stopped unexpectedly.",
            true,
        )
        .with_diagnostics(error.to_string())
    })?
}

#[tauri::command]
pub async fn use_embedded_translation_subtitle(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    stream_id: StreamId,
) -> Result<SubtitleSourceV1, AppErrorV1> {
    let media = state.media.clone();
    let core = state.core.clone();
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let extracted = media.extract_embedded_subtitle(&session_id, &stream_id)?;
        let source = core.register_embedded_translation_subtitle(
            &session_id,
            &extracted.path,
            extracted.stream_id,
            extracted.language,
        )?;
        if core.session(&session_id)?.source_scope_persistence
            == contracts::SourceScopePersistenceV1::UserApprovedPersistent
        {
            let (_, fingerprint, _) = media.source_identity_context(&session_id)?;
            store_translation_subtitle_binding(
                storage.as_ref(),
                &fingerprint,
                &extracted.path,
                &source,
            )?;
        }
        Ok(source)
    })
    .await
    .map_err(|error| {
        AppErrorV1::new(
            contracts::error_codes::CONVERSION_FAILED,
            "The translation subtitle extraction worker stopped unexpectedly.",
            true,
        )
        .with_diagnostics(error.to_string())
    })?
}

#[tauri::command]
pub fn remove_translation_subtitle(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    source_id: SubtitleSourceId,
) -> Result<(), AppErrorV1> {
    let (_, fingerprint, _) = state.media.source_identity_context(&session_id)?;
    state
        .core
        .unregister_translation_subtitle(&session_id, &source_id)?;
    state
        .storage
        .remove_translation_subtitle_binding(&fingerprint)?;
    Ok(())
}

#[tauri::command]
pub async fn subtitle_window(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    source_id: SubtitleSourceId,
    position_us: i64,
    revision: u64,
) -> Result<SubtitleWindowV1, AppErrorV1> {
    let core = state.core.clone();
    tauri::async_runtime::spawn_blocking(move || {
        core.subtitle_window(&session_id, &source_id, position_us, revision)
    })
    .await
    .map_err(|error| {
        AppErrorV1::new(
            contracts::error_codes::DICTIONARY_UNAVAILABLE,
            "The subtitle analysis worker stopped unexpectedly.",
            true,
        )
        .with_diagnostics(error.to_string())
    })?
}

#[tauri::command]
pub fn translation_window(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
    source_id: SubtitleSourceId,
    position_us: i64,
    revision: u64,
) -> Result<TranslationWindowV1, AppErrorV1> {
    state
        .core
        .translation_window(&session_id, &source_id, position_us, revision)
}

#[tauri::command]
pub fn create_draft(
    state: State<'_, RuntimeState>,
    selection: DraftSelection,
) -> Result<CardDraftV1, AppErrorV1> {
    state.core.create_draft(&selection)
}

#[tauri::command]
pub async fn publish_card(
    state: State<'_, RuntimeState>,
    request: CreateCardRequestV1,
) -> Result<CreateCardResultV1, AppErrorV1> {
    let mining = state.mining.read().clone();
    let result = tauri::async_runtime::spawn_blocking(move || mining.publish(request))
        .await
        .map_err(|error| {
            AppErrorV1::new(
                contracts::error_codes::CONVERSION_FAILED,
                "The card publishing worker stopped unexpectedly.",
                true,
            )
            .with_diagnostics(error.to_string())
        })?;
    if let Ok(published) = &result {
        state.recovery_report.write().clear_job(&published.job_id);
    }
    result
}

#[tauri::command]
pub fn close_media_session(
    state: State<'_, RuntimeState>,
    session_id: MediaSessionId,
) -> Result<(), AppErrorV1> {
    state.core.close_session(&session_id)
}

#[tauri::command]
pub fn settings(state: State<'_, RuntimeState>) -> UserSettingsV1 {
    state.settings.read().clone()
}

#[tauri::command]
pub fn save_settings(
    state: State<'_, RuntimeState>,
    settings: UserSettingsV1,
) -> Result<UserSettingsV1, AppErrorV1> {
    validate_user_settings(&settings)?;
    state
        .storage
        .put_setting(crate::SETTINGS_KEY, 1, &settings)?;
    state.media.set_cache_policy(cache_policy(&settings));
    state
        .media
        .set_audio_language_priority(settings.preferred_audio_languages.clone())?;
    let mining = build_mining_coordinator(
        state.core.clone(),
        state.media.clone(),
        state.storage.clone(),
        state.asset_store.clone(),
        &settings,
    );
    *state.mining.write() = mining;
    *state.settings.write() = settings.clone();
    Ok(settings)
}

#[tauri::command]
pub async fn choose_ffmpeg(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<ToolSelectionResultV1, AppErrorV1> {
    choose_media_tool(&app, &state, FFMPEG_PATH_KEY, "FFmpeg", "ffmpeg").await
}

#[tauri::command]
pub async fn choose_ffprobe(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<ToolSelectionResultV1, AppErrorV1> {
    choose_media_tool(&app, &state, FFPROBE_PATH_KEY, "FFprobe", "ffprobe").await
}

async fn choose_media_tool(
    app: &AppHandle,
    state: &State<'_, RuntimeState>,
    setting_key: &str,
    component: &str,
    expected_name: &str,
) -> Result<ToolSelectionResultV1, AppErrorV1> {
    let selected = app
        .dialog()
        .file()
        .add_filter("Executable", &["exe"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(ToolSelectionResultV1 {
            component: component.into(),
            configured: false,
            restart_required: false,
        });
    };
    let path = selected.into_path().map_err(scope_error)?;
    let checked_path = path.clone();
    let expected = expected_name.to_owned();
    tauri::async_runtime::spawn_blocking(move || inspect_tool(&checked_path, &expected))
        .await
        .map_err(worker_error)??;
    state.storage.put_setting(setting_key, 1, &path)?;
    Ok(ToolSelectionResultV1 {
        component: component.into(),
        configured: true,
        restart_required: true,
    })
}

#[tauri::command]
pub async fn choose_dictionary(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<ToolSelectionResultV1, AppErrorV1> {
    let selected = app
        .dialog()
        .file()
        .add_filter("Migaku JMdict database", &["sqlite", "db"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(ToolSelectionResultV1 {
            component: "JMdict dictionary".into(),
            configured: false,
            restart_required: false,
        });
    };
    let source = selected.into_path().map_err(scope_error)?;
    let dictionary_root = state.data_dir.join("dictionaries");
    let storage = state.storage.clone();
    let destination =
        tauri::async_runtime::spawn_blocking(move || import_dictionary(&source, &dictionary_root))
            .await
            .map_err(worker_error)??;
    storage.put_setting(DICTIONARY_PATH_KEY, 1, &destination)?;
    Ok(ToolSelectionResultV1 {
        component: "JMdict dictionary".into(),
        configured: true,
        restart_required: true,
    })
}

#[tauri::command]
pub async fn health(state: State<'_, RuntimeState>) -> Result<AppHealthV1, AppErrorV1> {
    let mut health = state.base_health.clone();
    health.checks.push(state.media.cache_health());
    let mining = state.mining.read().clone();
    let storage = state.storage.clone();
    let recovery_report = state.recovery_report.clone();
    let profile_id = state.settings.read().card_profile.profile_id.clone();
    let dynamic = tauri::async_runtime::spawn_blocking(move || {
        let anki = match mining.validate_profile(&profile_id) {
            Ok(()) => DependencyHealthV1 {
                component: "AnkiConnect and card mapping".into(),
                available: true,
                version: Some("API 6+".into()),
                action: None,
                error: None,
            },
            Err(error) => DependencyHealthV1 {
                component: "AnkiConnect and card mapping".into(),
                available: false,
                version: None,
                action: Some(
                    "Start Anki and review the deck, note type, and field mapping.".into(),
                ),
                error: Some(error),
            },
        };
        let recovery_state = (|| {
            Ok::<_, AppErrorV1>((
                storage.pending_publish_attempts(1_000)?,
                storage.recoverable_jobs(1_000)?,
                storage.failed_jobs(1_000)?,
                storage.pending_cancelled_publish_cleanups(1_000)?,
                storage.cancelled_publish_cleanup_count()?,
            ))
        })();
        let jobs = match recovery_state {
            Ok((pending, durable, failed, cleanups, cleanup_count)) => {
                let durable_count = durable
                    .iter()
                    .filter(|job| job.kind == "card_publish")
                    .count();
                let failed = failed
                    .into_iter()
                    .filter(|job| job.kind == "card_publish")
                    .collect::<Vec<_>>();
                let recovery = recovery_report.read().clone();
                let has_attention = !pending.is_empty()
                    || durable_count > 0
                    || !failed.is_empty()
                    || cleanup_count > 0;
                let issue_matches = |job_id: &contracts::JobId| {
                    durable.iter().any(|job| &job.job_id == job_id)
                        || failed.iter().any(|job| &job.job_id == job_id)
                        || cleanups.iter().any(|job| &job.job_id == job_id)
                };
                let recovery_error = recovery
                    .errors
                    .iter()
                    .find(|issue| issue.job_id.as_ref().is_none_or(issue_matches))
                    .map(|issue| issue.error.clone())
                    .or_else(|| {
                        failed.iter().find_map(|job| {
                            job.terminal_error_json
                                .as_deref()
                                .and_then(|json| serde_json::from_str(json).ok())
                        })
                    })
                    .or_else(|| {
                        cleanups.iter().find_map(|job| {
                            job.cleanup_error_json
                                .as_deref()
                                .and_then(|json| serde_json::from_str(json).ok())
                        })
                    })
                    .or_else(|| {
                        (cleanup_count > 0).then(|| {
                            AppErrorV1::new(
                                contracts::error_codes::CONVERSION_FAILED,
                                "Cancelled mining assets are still awaiting durable cleanup.",
                                true,
                            )
                        })
                    });
                if !has_attention && !recovery.errors.is_empty() {
                    let mut stored_recovery = recovery_report.write();
                    stored_recovery.attempted = 0;
                    stored_recovery.completed = 0;
                    stored_recovery.errors.clear();
                }
                DependencyHealthV1 {
                component: "Pending mining/publish jobs".into(),
                available: !has_attention && recovery_error.is_none(),
                version: Some(format!(
                    "{} attempt stages, {durable_count} durable requests, {} terminal failures, {cleanup_count} cancelled cleanups; startup recovery {}/{} completed",
                    pending.len(), failed.len(), recovery.completed, recovery.attempted
                )),
                action: (has_attention || recovery_error.is_some())
                    .then(|| "Reopen remembered source media or restore Anki, then restart or retry the publish.".into()),
                error: recovery_error,
            }
            }
            Err(error) => DependencyHealthV1 {
                component: "Pending mining/publish jobs".into(),
                available: false,
                version: None,
                action: Some("Repair or restore the user database backup.".into()),
                error: Some(error),
            },
        };
        [anki, jobs]
    })
    .await
    .unwrap_or_else(|error| {
        [
            DependencyHealthV1 {
                component: "AnkiConnect and card mapping".into(),
                available: false,
                version: None,
                action: Some("Retry the health check.".into()),
                error: Some(worker_error(error)),
            },
            DependencyHealthV1 {
                component: "Pending mining/publish jobs".into(),
                available: false,
                version: None,
                action: Some("Retry the health check.".into()),
                error: None,
            },
        ]
    });
    health.checks.extend(dynamic);
    Ok(health)
}

fn register_and_bind(
    state: &State<'_, RuntimeState>,
    session_id: &MediaSessionId,
    path: &std::path::Path,
    origin: SubtitleOriginV1,
) -> Result<SubtitleSourceV1, AppErrorV1> {
    let source = state.core.register_subtitle(session_id, path, origin)?;
    if state.core.session(session_id)?.source_scope_persistence
        == contracts::SourceScopePersistenceV1::UserApprovedPersistent
    {
        let (_, fingerprint, _) = state.media.source_identity_context(session_id)?;
        store_subtitle_binding(state.storage.as_ref(), &fingerprint, path, &source)?;
    }
    Ok(source)
}

fn validate_active_playlist_session(
    playlist: &PlaylistManager,
    media: &media_engine::MediaEngine,
    item: &ResolvedPlaylistItem,
    session_id: &MediaSessionId,
) -> Result<(), AppErrorV1> {
    let (source_path, _, _) = media.source_identity_context(session_id)?;
    if !playlist.is_current_item(&item.item_id) || source_path != item.path {
        return Err(AppErrorV1::new(
            contracts::error_codes::STALE_REVISION,
            "The active media session no longer belongs to this playlist item.",
            true,
        ));
    }
    Ok(())
}

fn register_playlist_subtitle(
    core: &AppCore,
    storage: &storage::Storage,
    session_id: &MediaSessionId,
    fingerprint: &str,
    subtitle: &ResolvedPlaylistSubtitle,
    role: SubtitleRoleV1,
) -> Result<SubtitleSourceV1, AppErrorV1> {
    let source = match role {
        SubtitleRoleV1::Japanese => {
            core.register_subtitle(session_id, &subtitle.path, subtitle.origin)?
        }
        SubtitleRoleV1::Translation => {
            core.register_translation_subtitle(session_id, &subtitle.path, subtitle.origin)?
        }
    };
    match role {
        SubtitleRoleV1::Japanese => {
            store_subtitle_binding(storage, fingerprint, &subtitle.path, &source)?;
        }
        SubtitleRoleV1::Translation => {
            store_translation_subtitle_binding(storage, fingerprint, &subtitle.path, &source)?;
        }
    }
    Ok(source)
}

fn remove_playlist_subtitle_binding(
    storage: &storage::Storage,
    fingerprint: &str,
    role: SubtitleRoleV1,
) -> Result<(), AppErrorV1> {
    match role {
        SubtitleRoleV1::Japanese => storage.remove_subtitle_binding(fingerprint)?,
        SubtitleRoleV1::Translation => storage.remove_translation_subtitle_binding(fingerprint)?,
    };
    Ok(())
}

fn register_translation_and_bind(
    state: &State<'_, RuntimeState>,
    session_id: &MediaSessionId,
    path: &std::path::Path,
    origin: SubtitleOriginV1,
) -> Result<SubtitleSourceV1, AppErrorV1> {
    let source = state
        .core
        .register_translation_subtitle(session_id, path, origin)?;
    if state.core.session(session_id)?.source_scope_persistence
        == contracts::SourceScopePersistenceV1::UserApprovedPersistent
    {
        let (_, fingerprint, _) = state.media.source_identity_context(session_id)?;
        store_translation_subtitle_binding(state.storage.as_ref(), &fingerprint, path, &source)?;
    }
    Ok(source)
}

fn store_subtitle_binding(
    storage: &storage::Storage,
    fingerprint: &str,
    path: &std::path::Path,
    source: &SubtitleSourceV1,
) -> Result<(), AppErrorV1> {
    storage.bind_subtitle(
        fingerprint,
        &source.source_version,
        &StoredSubtitleBinding {
            path: path.to_path_buf(),
            origin: source.origin,
            embedded_stream_id: source.embedded_stream_id.clone(),
            language: source.language.clone(),
        },
    )
}

fn store_translation_subtitle_binding(
    storage: &storage::Storage,
    fingerprint: &str,
    path: &std::path::Path,
    source: &SubtitleSourceV1,
) -> Result<(), AppErrorV1> {
    storage.bind_translation_subtitle(
        fingerprint,
        &source.source_version,
        &StoredSubtitleBinding {
            path: path.to_path_buf(),
            origin: source.origin,
            embedded_stream_id: source.embedded_stream_id.clone(),
            language: source.language.clone(),
        },
    )
}

fn subtitle_candidate_id(fingerprint: &str, path: &std::path::Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"migaku-adjacent-subtitle-v1\0");
    hasher.update(fingerprint.as_bytes());
    hasher.update([0]);
    hasher.update(path.as_os_str().to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())
}

fn validate_candidate_id(candidate_id: &str) -> Result<(), AppErrorV1> {
    if candidate_id.len() != 64
        || !candidate_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(AppErrorV1::new(
            contracts::error_codes::INVALID_REQUEST,
            "The adjacent subtitle selection was invalid.",
            false,
        ));
    }
    Ok(())
}

fn import_dictionary(
    source: &std::path::Path,
    destination_root: &std::path::Path,
) -> Result<std::path::PathBuf, AppErrorV1> {
    SqliteDictionary::open(source)?;
    std::fs::create_dir_all(destination_root).map_err(storage_error)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(storage_error)?
        .as_nanos();
    let partial = destination_root.join(format!("dictionary-{nonce}.part"));
    let destination = destination_root.join(format!("dictionary-{nonce}.sqlite"));
    std::fs::copy(source, &partial).map_err(storage_error)?;
    if let Err(error) = SqliteDictionary::open(&partial) {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&partial, &destination) {
        let _ = std::fs::remove_file(&partial);
        return Err(storage_error(error));
    }
    Ok(destination)
}

fn scope_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        contracts::error_codes::MEDIA_SCOPE_DENIED,
        "The selected file could not be authorized.",
        true,
    )
    .with_diagnostics(error.to_string())
}

fn storage_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        contracts::error_codes::STORAGE_FAILED,
        "The selected dependency could not be stored safely.",
        true,
    )
    .with_diagnostics(error.to_string())
}

fn worker_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        contracts::error_codes::INVALID_REQUEST,
        "A background setup check stopped unexpectedly.",
        true,
    )
    .with_diagnostics(error.to_string())
}

fn playlist_worker_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        contracts::error_codes::INVALID_REQUEST,
        "A background playlist operation stopped unexpectedly.",
        true,
    )
    .with_diagnostics(error.to_string())
}
