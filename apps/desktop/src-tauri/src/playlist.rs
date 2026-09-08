use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use contracts::{
    AppErrorV1, MediaSessionId, MediaSessionV1, SubtitleOriginV1, SubtitleSourceV1, error_codes,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use storage::Storage;

const PLAYLIST_STORAGE_KEY: &str = "playlist.folder.v1";
const PLAYLIST_STORAGE_VERSION: u32 = 1;
const MAX_PLAYLIST_ITEMS: usize = 5_000;
const MAX_SUBTITLE_FILES: usize = 10_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistScanModeV1 {
    AutoImport,
    Preview,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleRoleV1 {
    Japanese,
    Translation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistSubtitleKindV1 {
    Auto,
    Override,
    Ambiguous,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlaylistSubtitleV1 {
    pub kind: PlaylistSubtitleKindV1,
    pub display_name: Option<String>,
    pub confidence: Option<u8>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlaylistItemV1 {
    pub item_id: String,
    pub display_name: String,
    pub ordinal: usize,
    pub japanese_subtitle: PlaylistSubtitleV1,
    pub translation_subtitle: PlaylistSubtitleV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistWatchStatusV1 {
    Watching,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlaylistWatchStateV1 {
    pub status: PlaylistWatchStatusV1,
    pub last_scan_unix_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlaylistV1 {
    pub playlist_id: String,
    pub display_name: String,
    pub revision: u64,
    pub scan_revision: u64,
    pub change_id: String,
    pub items: Vec<PlaylistItemV1>,
    pub current_item_id: Option<String>,
    pub watch_state: PlaylistWatchStateV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlaylistDiscoveryItemV1 {
    pub item_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlaylistScanResultV1 {
    pub playlist: PlaylistV1,
    pub pending_items: Vec<PlaylistDiscoveryItemV1>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlaylistSelectionV1 {
    pub playlist: PlaylistV1,
    pub session: MediaSessionV1,
    pub japanese_subtitle: Option<SubtitleSourceV1>,
    pub translation_subtitle: Option<SubtitleSourceV1>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlaylistSubtitleMutationV1 {
    pub playlist: PlaylistV1,
    pub source: Option<SubtitleSourceV1>,
}

#[derive(Clone, Debug)]
pub struct ResolvedPlaylistSubtitle {
    pub path: PathBuf,
    pub origin: SubtitleOriginV1,
}

#[derive(Clone, Debug)]
pub struct ResolvedPlaylistItem {
    pub item_id: String,
    pub path: PathBuf,
    pub japanese_subtitle: Option<ResolvedPlaylistSubtitle>,
    pub translation_subtitle: Option<ResolvedPlaylistSubtitle>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredPlaylistItem {
    item_id: String,
    path: PathBuf,
    display_name: String,
    size_bytes: u64,
    modified_unix_nanos: u128,
    automatic_japanese_subtitle: StoredSubtitleMatch,
    automatic_translation_subtitle: StoredSubtitleMatch,
    japanese_subtitle_override: Option<PathBuf>,
    translation_subtitle_override: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredSubtitleMatch {
    Matched {
        path: PathBuf,
        confidence: u8,
        reason: String,
    },
    Ambiguous {
        confidence: u8,
        reason: String,
    },
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredPlaylist {
    playlist_id: String,
    directory: PathBuf,
    display_name: String,
    revision: u64,
    scan_revision: u64,
    change_id: String,
    items: Vec<StoredPlaylistItem>,
    ignored_item_ids: BTreeSet<String>,
    current_item_id: Option<String>,
    last_scan_unix_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScannedVideo {
    item_id: String,
    path: PathBuf,
    display_name: String,
    size_bytes: u64,
    modified_unix_nanos: u128,
    automatic_japanese_subtitle: StoredSubtitleMatch,
    automatic_translation_subtitle: StoredSubtitleMatch,
}

impl ScannedVideo {
    fn into_stored(self) -> StoredPlaylistItem {
        StoredPlaylistItem {
            item_id: self.item_id,
            path: self.path,
            display_name: self.display_name,
            size_bytes: self.size_bytes,
            modified_unix_nanos: self.modified_unix_nanos,
            automatic_japanese_subtitle: self.automatic_japanese_subtitle,
            automatic_translation_subtitle: self.automatic_translation_subtitle,
            japanese_subtitle_override: None,
            translation_subtitle_override: None,
        }
    }
}

#[derive(Clone, Debug)]
struct DirectoryScan {
    videos: Vec<ScannedVideo>,
    change_id: String,
}

#[derive(Default, Debug)]
struct PlaylistRuntime {
    playlist: Option<StoredPlaylist>,
    pending: BTreeMap<String, ScannedVideo>,
}

#[derive(Debug)]
pub struct PlaylistManager {
    storage: Arc<Storage>,
    runtime: RwLock<PlaylistRuntime>,
}

impl PlaylistManager {
    pub fn load(storage: Arc<Storage>) -> Result<Self, AppErrorV1> {
        let playlist = match storage.get_setting::<StoredPlaylist>(PLAYLIST_STORAGE_KEY)? {
            Some((PLAYLIST_STORAGE_VERSION, playlist)) => {
                validate_stored_playlist(&playlist)?;
                Some(playlist)
            }
            Some(_) => {
                return Err(storage_error(
                    "The saved playlist was created by an unsupported application version.",
                ));
            }
            None => None,
        };
        Ok(Self {
            storage,
            runtime: RwLock::new(PlaylistRuntime {
                playlist,
                pending: BTreeMap::new(),
            }),
        })
    }

    pub fn import_folder(
        &self,
        selected_path: &Path,
        mode: PlaylistScanModeV1,
    ) -> Result<PlaylistScanResultV1, AppErrorV1> {
        let directory = canonical_folder(selected_path)?;
        let scan = scan_directory(&directory)?;
        let display_name = safe_display_name(&directory, "Selected folder");
        let (items, pending) = match mode {
            PlaylistScanModeV1::AutoImport => (
                scan.videos
                    .iter()
                    .cloned()
                    .map(ScannedVideo::into_stored)
                    .collect(),
                BTreeMap::new(),
            ),
            PlaylistScanModeV1::Preview => (
                Vec::new(),
                scan.videos
                    .iter()
                    .cloned()
                    .map(|video| (video.item_id.clone(), video))
                    .collect(),
            ),
        };
        let playlist = StoredPlaylist {
            playlist_id: stable_id(b"migaku-playlist-v1\0", &directory),
            directory,
            display_name,
            revision: 1,
            scan_revision: 1,
            change_id: scan.change_id,
            items,
            ignored_item_ids: BTreeSet::new(),
            current_item_id: None,
            last_scan_unix_ms: unix_time_ms(),
        };
        self.persist(&playlist)?;
        let public = public_playlist(&playlist);
        let pending_items = pending
            .values()
            .map(|item| PlaylistDiscoveryItemV1 {
                item_id: item.item_id.clone(),
                display_name: item.display_name.clone(),
            })
            .collect();
        *self.runtime.write() = PlaylistRuntime {
            playlist: Some(playlist),
            pending,
        };
        Ok(PlaylistScanResultV1 {
            playlist: public,
            pending_items,
        })
    }

    pub fn current(&self) -> Option<PlaylistV1> {
        self.runtime.read().playlist.as_ref().map(public_playlist)
    }

    pub fn clear_current(&self) -> Result<Option<PlaylistV1>, AppErrorV1> {
        let mut runtime = self.runtime.write();
        let Some(playlist) = runtime.playlist.as_mut() else {
            return Ok(None);
        };
        if playlist.current_item_id.take().is_some() {
            playlist.revision = playlist.revision.saturating_add(1);
            self.persist(playlist)?;
        }
        Ok(Some(public_playlist(playlist)))
    }

    pub fn rescan(&self, mode: PlaylistScanModeV1) -> Result<PlaylistScanResultV1, AppErrorV1> {
        let (playlist_id, directory) = {
            let runtime = self.runtime.read();
            let playlist = runtime.playlist.as_ref().ok_or_else(no_playlist_error)?;
            (playlist.playlist_id.clone(), playlist.directory.clone())
        };
        let scan = scan_directory(&directory)?;
        let mut runtime = self.runtime.write();
        let PlaylistRuntime { playlist, pending } = &mut *runtime;
        let playlist = playlist.as_mut().ok_or_else(no_playlist_error)?;
        if playlist.playlist_id != playlist_id {
            return Err(stale_error(
                "The selected playlist changed while its folder was being scanned.",
            ));
        }

        let scan_identity_changed = playlist.change_id != scan.change_id;
        if scan_identity_changed {
            playlist.scan_revision = playlist.scan_revision.saturating_add(1);
            playlist.change_id.clone_from(&scan.change_id);
        }
        playlist.last_scan_unix_ms = unix_time_ms();

        let mut scanned_by_id = scan
            .videos
            .into_iter()
            .map(|video| (video.item_id.clone(), video))
            .collect::<HashMap<_, _>>();
        let mut playlist_changed = false;
        let mut retained = Vec::with_capacity(playlist.items.len());
        for mut item in playlist.items.drain(..) {
            let Some(scanned) = scanned_by_id.remove(&item.item_id) else {
                playlist_changed = true;
                continue;
            };
            if update_from_scan(&mut item, &scanned) {
                playlist_changed = true;
            }
            retained.push(item);
        }
        playlist.items = retained;
        if playlist
            .current_item_id
            .as_ref()
            .is_some_and(|current| !playlist.items.iter().any(|item| &item.item_id == current))
        {
            playlist.current_item_id = None;
            playlist_changed = true;
        }

        let mut additions = scanned_by_id
            .into_values()
            .filter(|video| !playlist.ignored_item_ids.contains(&video.item_id))
            .collect::<Vec<_>>();
        additions.sort_by(|left, right| natural_path_cmp(&left.path, &right.path));
        match mode {
            PlaylistScanModeV1::AutoImport => {
                if !additions.is_empty() {
                    playlist_changed = true;
                }
                playlist
                    .items
                    .extend(additions.into_iter().map(ScannedVideo::into_stored));
                pending.clear();
            }
            PlaylistScanModeV1::Preview => {
                *pending = additions
                    .into_iter()
                    .map(|video| (video.item_id.clone(), video))
                    .collect();
            }
        }
        if playlist_changed {
            playlist.revision = playlist.revision.saturating_add(1);
        }
        if playlist_changed || scan_identity_changed {
            self.persist(playlist)?;
        }
        let public = public_playlist(playlist);
        let pending_items = pending
            .values()
            .map(|item| PlaylistDiscoveryItemV1 {
                item_id: item.item_id.clone(),
                display_name: item.display_name.clone(),
            })
            .collect();
        Ok(PlaylistScanResultV1 {
            playlist: public,
            pending_items,
        })
    }

    pub fn import_discoveries(&self, item_ids: &[String]) -> Result<PlaylistV1, AppErrorV1> {
        if item_ids.len() > MAX_PLAYLIST_ITEMS {
            return Err(invalid_request(
                "Too many playlist discoveries were selected.",
            ));
        }
        for item_id in item_ids {
            validate_id(item_id)?;
        }
        // Refresh the staging set first so stale paths cannot be imported after a
        // rename, replacement, or folder switch.
        self.rescan(PlaylistScanModeV1::Preview)?;
        let mut runtime = self.runtime.write();
        let mut selected = Vec::with_capacity(item_ids.len());
        for item_id in item_ids {
            let item = runtime.pending.remove(item_id).ok_or_else(|| {
                stale_error("A selected playlist discovery is no longer available.")
            })?;
            selected.push(item);
        }
        selected.sort_by(|left, right| natural_path_cmp(&left.path, &right.path));
        let playlist = runtime.playlist.as_mut().ok_or_else(no_playlist_error)?;
        if !selected.is_empty() {
            for item in selected {
                playlist.ignored_item_ids.remove(&item.item_id);
                playlist.items.push(item.into_stored());
            }
            playlist.revision = playlist.revision.saturating_add(1);
            self.persist(playlist)?;
        }
        Ok(public_playlist(playlist))
    }

    pub fn resolve_item(&self, item_id: &str) -> Result<ResolvedPlaylistItem, AppErrorV1> {
        validate_id(item_id)?;
        let runtime = self.runtime.read();
        let playlist = runtime.playlist.as_ref().ok_or_else(no_playlist_error)?;
        let item = playlist
            .items
            .iter()
            .find(|item| item.item_id == item_id)
            .ok_or_else(|| stale_error("The selected playlist item is no longer available."))?;
        validate_playlist_media_path(&playlist.directory, &item.path)?;
        Ok(ResolvedPlaylistItem {
            item_id: item.item_id.clone(),
            path: item.path.clone(),
            japanese_subtitle: resolved_subtitle_choice(
                item.japanese_subtitle_override.as_ref(),
                &item.automatic_japanese_subtitle,
            ),
            translation_subtitle: resolved_subtitle_choice(
                item.translation_subtitle_override.as_ref(),
                &item.automatic_translation_subtitle,
            ),
        })
    }

    pub fn is_current_item(&self, item_id: &str) -> bool {
        self.runtime
            .read()
            .playlist
            .as_ref()
            .and_then(|playlist| playlist.current_item_id.as_deref())
            == Some(item_id)
    }

    pub fn select_item(
        &self,
        item_id: &str,
        _session_id: &MediaSessionId,
    ) -> Result<PlaylistV1, AppErrorV1> {
        validate_id(item_id)?;
        let mut runtime = self.runtime.write();
        let playlist = runtime.playlist.as_mut().ok_or_else(no_playlist_error)?;
        if !playlist.items.iter().any(|item| item.item_id == item_id) {
            return Err(stale_error(
                "The selected playlist item is no longer available.",
            ));
        }
        if playlist.current_item_id.as_deref() != Some(item_id) {
            playlist.current_item_id = Some(item_id.to_owned());
            playlist.revision = playlist.revision.saturating_add(1);
            self.persist(playlist)?;
        }
        Ok(public_playlist(playlist))
    }

    pub fn reorder_item(&self, item_id: &str, new_index: usize) -> Result<PlaylistV1, AppErrorV1> {
        validate_id(item_id)?;
        let mut runtime = self.runtime.write();
        let playlist = runtime.playlist.as_mut().ok_or_else(no_playlist_error)?;
        if new_index >= playlist.items.len() {
            return Err(invalid_request(
                "The requested playlist position is outside the playlist.",
            ));
        }
        let current_index = playlist
            .items
            .iter()
            .position(|item| item.item_id == item_id)
            .ok_or_else(|| stale_error("The playlist item is no longer available."))?;
        if current_index != new_index {
            let item = playlist.items.remove(current_index);
            playlist.items.insert(new_index, item);
            playlist.revision = playlist.revision.saturating_add(1);
            self.persist(playlist)?;
        }
        Ok(public_playlist(playlist))
    }

    pub fn remove_item(&self, item_id: &str) -> Result<PlaylistV1, AppErrorV1> {
        validate_id(item_id)?;
        let mut runtime = self.runtime.write();
        let playlist = runtime.playlist.as_mut().ok_or_else(no_playlist_error)?;
        let index = playlist
            .items
            .iter()
            .position(|item| item.item_id == item_id)
            .ok_or_else(|| stale_error("The playlist item is no longer available."))?;
        playlist.items.remove(index);
        playlist.ignored_item_ids.insert(item_id.to_owned());
        if playlist.current_item_id.as_deref() == Some(item_id) {
            playlist.current_item_id = None;
        }
        playlist.revision = playlist.revision.saturating_add(1);
        self.persist(playlist)?;
        Ok(public_playlist(playlist))
    }

    pub fn canonical_subtitle_override(&self, path: &Path) -> Result<PathBuf, AppErrorV1> {
        validate_subtitle_path(path)
    }

    pub fn subtitle_path_after_clear(
        &self,
        item_id: &str,
        role: SubtitleRoleV1,
    ) -> Result<Option<ResolvedPlaylistSubtitle>, AppErrorV1> {
        validate_id(item_id)?;
        let runtime = self.runtime.read();
        let playlist = runtime.playlist.as_ref().ok_or_else(no_playlist_error)?;
        let item = playlist
            .items
            .iter()
            .find(|item| item.item_id == item_id)
            .ok_or_else(|| stale_error("The playlist item is no longer available."))?;
        Ok(resolved_subtitle_choice(
            None,
            match role {
                SubtitleRoleV1::Japanese => &item.automatic_japanese_subtitle,
                SubtitleRoleV1::Translation => &item.automatic_translation_subtitle,
            },
        ))
    }

    pub fn set_subtitle_override(
        &self,
        item_id: &str,
        role: SubtitleRoleV1,
        path: Option<PathBuf>,
    ) -> Result<PlaylistV1, AppErrorV1> {
        validate_id(item_id)?;
        let mut runtime = self.runtime.write();
        let playlist = runtime.playlist.as_mut().ok_or_else(no_playlist_error)?;
        let item = playlist
            .items
            .iter_mut()
            .find(|item| item.item_id == item_id)
            .ok_or_else(|| stale_error("The playlist item is no longer available."))?;
        let target = match role {
            SubtitleRoleV1::Japanese => &mut item.japanese_subtitle_override,
            SubtitleRoleV1::Translation => &mut item.translation_subtitle_override,
        };
        if *target != path {
            *target = path;
            playlist.revision = playlist.revision.saturating_add(1);
            self.persist(playlist)?;
        }
        Ok(public_playlist(playlist))
    }

    fn persist(&self, playlist: &StoredPlaylist) -> Result<(), AppErrorV1> {
        self.storage
            .put_setting(PLAYLIST_STORAGE_KEY, PLAYLIST_STORAGE_VERSION, playlist)
    }
}

fn scan_directory(directory: &Path) -> Result<DirectoryScan, AppErrorV1> {
    let canonical = canonical_folder(directory)?;
    let mut videos = Vec::new();
    let mut subtitles = Vec::new();
    for entry in fs::read_dir(&canonical).map_err(folder_read_error)? {
        let entry = entry.map_err(folder_read_error)?;
        let file_type = entry.file_type().map_err(folder_read_error)?;
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if is_video_path(&path) {
            if videos.len() >= MAX_PLAYLIST_ITEMS {
                return Err(invalid_request(
                    "This folder contains more than 5,000 supported videos. Choose a smaller folder.",
                ));
            }
            let path = canonical_child(&canonical, &path)?;
            let metadata = path.metadata().map_err(folder_read_error)?;
            videos.push(ScannedVideo {
                item_id: stable_id(b"migaku-playlist-item-v1\0", &path),
                display_name: safe_display_name(&path, "Local video"),
                path,
                size_bytes: metadata.len(),
                modified_unix_nanos: modified_unix_nanos(&metadata),
                automatic_japanese_subtitle: StoredSubtitleMatch::None,
                automatic_translation_subtitle: StoredSubtitleMatch::None,
            });
        } else if is_subtitle_path(&path) {
            if subtitles.len() >= MAX_SUBTITLE_FILES {
                return Err(invalid_request(
                    "This folder contains too many subtitle files to scan safely.",
                ));
            }
            subtitles.push(canonical_child(&canonical, &path)?);
        }
    }
    videos.sort_by(|left, right| natural_path_cmp(&left.path, &right.path));
    subtitles.sort_by(|left, right| natural_path_cmp(left, right));
    for video in &mut videos {
        video.automatic_japanese_subtitle =
            matched_subtitle(&video.path, &subtitles, SubtitleRoleV1::Japanese);
        video.automatic_translation_subtitle =
            matched_subtitle(&video.path, &subtitles, SubtitleRoleV1::Translation);
    }
    let change_id = directory_change_id(&videos, &subtitles);
    Ok(DirectoryScan { videos, change_id })
}

fn matched_subtitle(
    video: &Path,
    subtitles: &[PathBuf],
    role: SubtitleRoleV1,
) -> StoredSubtitleMatch {
    let Some(video_stem) = video.file_stem() else {
        return StoredSubtitleMatch::None;
    };
    let video_stem = video_stem.to_string_lossy().to_lowercase();
    let video_identity = title_identity(&video_stem);
    let mut ranked = subtitles
        .iter()
        .filter_map(|subtitle| {
            let subtitle_stem = subtitle.file_stem()?.to_string_lossy().to_lowercase();
            let subtitle_identity = title_identity(&subtitle_stem);
            let language = subtitle_language(&subtitle_stem, role);
            let exact_stem = subtitle_stem == video_stem;
            let (score, reason) = match_title(
                &video_identity,
                &subtitle_identity,
                exact_stem,
                language,
                role,
            )?;
            Some((score, reason, subtitle))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| natural_path_cmp(left.2, right.2))
    });
    let Some((best_score, best_reason, _)) = ranked.first() else {
        return StoredSubtitleMatch::None;
    };
    let mut best = ranked
        .iter()
        .filter(|(score, _, _)| score == best_score)
        .map(|(_, _, path)| (*path).clone());
    let Some(selected) = best.next() else {
        return StoredSubtitleMatch::None;
    };
    if best.next().is_some() {
        StoredSubtitleMatch::Ambiguous {
            confidence: *best_score,
            reason: format!("Multiple subtitles share the best match: {best_reason}"),
        }
    } else {
        StoredSubtitleMatch::Matched {
            path: selected,
            confidence: *best_score,
            reason: best_reason.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TitleIdentity {
    title: String,
    season: Option<u32>,
    episode: Option<u32>,
}

fn title_identity(stem: &str) -> TitleIdentity {
    let unbracketed = strip_bracket_groups(stem);
    let raw_tokens = unbracketed
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let explicit_episode = raw_tokens
        .iter()
        .find_map(|token| parse_episode_token(token));
    let standalone_episode = explicit_episode.is_none().then(|| {
        raw_tokens
            .iter()
            .rev()
            .find_map(|token| parse_standalone_episode(token))
    });
    let (season, episode) = explicit_episode
        .or_else(|| standalone_episode.flatten())
        .map_or((None, None), |(season, episode)| (season, Some(episode)));

    let mut title_tokens = Vec::new();
    for token in raw_tokens {
        if is_release_noise(&token) {
            break;
        }
        if parse_episode_token(&token).is_some()
            || episode.is_some_and(|episode| {
                parse_standalone_episode(&token).is_some_and(|(_, value)| value == episode)
            })
            || is_language_or_subtitle_tag(&token)
        {
            continue;
        }
        title_tokens.push(token);
    }
    TitleIdentity {
        title: title_tokens.join(" "),
        season,
        episode,
    }
}

fn match_title(
    video: &TitleIdentity,
    subtitle: &TitleIdentity,
    exact_stem: bool,
    language: Option<SubtitleRoleV1>,
    requested_role: SubtitleRoleV1,
) -> Option<(u8, String)> {
    if language.is_some_and(|language| language != requested_role)
        || (requested_role == SubtitleRoleV1::Translation && language.is_none())
    {
        return None;
    }
    if exact_stem && requested_role == SubtitleRoleV1::Japanese {
        return Some((
            90,
            "Exact video title fallback for Japanese subtitles".into(),
        ));
    }
    if video.title.is_empty() || subtitle.title.is_empty() || video.title != subtitle.title {
        return None;
    }
    match (video.episode, subtitle.episode) {
        (Some(left), Some(right)) if left != right => return None,
        (Some(_), None) | (None, Some(_)) => return None,
        _ => {}
    }
    if video.season.is_some() && subtitle.season.is_some() && video.season != subtitle.season {
        return None;
    }
    let mut confidence = 80_u8;
    let mut reason = "Normalized title match".to_owned();
    if video.episode.is_some() {
        confidence = confidence.saturating_add(10);
        reason.push_str(" with the same episode");
    }
    if language == Some(requested_role) {
        confidence = confidence.saturating_add(10);
        reason.push_str(match requested_role {
            SubtitleRoleV1::Japanese => " and Japanese language tag",
            SubtitleRoleV1::Translation => " and English language tag",
        });
    }
    (confidence >= 85).then_some((confidence, reason))
}

fn subtitle_language(stem: &str, requested_role: SubtitleRoleV1) -> Option<SubtitleRoleV1> {
    let mut japanese = false;
    let mut translation = false;
    for token in stem
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        japanese |= ["ja", "jpn", "jp", "japanese"]
            .iter()
            .any(|tag| token.eq_ignore_ascii_case(tag));
        translation |= ["en", "eng", "english"]
            .iter()
            .any(|tag| token.eq_ignore_ascii_case(tag));
    }
    match (japanese, translation) {
        (true, false) => Some(SubtitleRoleV1::Japanese),
        (false, true) => Some(SubtitleRoleV1::Translation),
        (true, true) => None,
        (false, false) if requested_role == SubtitleRoleV1::Japanese => None,
        (false, false) => None,
    }
}

fn parse_episode_token(token: &str) -> Option<(Option<u32>, u32)> {
    let token = token.to_ascii_lowercase();
    if let Some(season_episode) = token.strip_prefix('s')
        && let Some((season, episode)) = season_episode.split_once('e')
        && !season.is_empty()
        && !episode.is_empty()
    {
        return Some((Some(season.parse().ok()?), episode.parse().ok()?));
    }
    for prefix in ["episode", "ep", "e"] {
        if let Some(value) = token.strip_prefix(prefix)
            && !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Some((None, value.parse().ok()?));
        }
    }
    None
}

fn parse_standalone_episode(token: &str) -> Option<(Option<u32>, u32)> {
    if token.is_empty() || token.len() > 3 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = token.parse().ok()?;
    (value <= 999).then_some((None, value))
}

fn strip_bracket_groups(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut depth = 0_u32;
    for character in value.chars() {
        match character {
            '[' | '(' | '{' => depth = depth.saturating_add(1),
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => result.push(character),
            _ => {}
        }
    }
    result
}

fn is_release_noise(token: &str) -> bool {
    matches!(
        token,
        "2160p"
            | "1080p"
            | "720p"
            | "480p"
            | "uhd"
            | "hdr"
            | "dv"
            | "web"
            | "webdl"
            | "webrip"
            | "bluray"
            | "bdrip"
            | "h264"
            | "h265"
            | "x264"
            | "x265"
            | "hevc"
            | "av1"
            | "aac"
            | "flac"
            | "opus"
            | "ddp"
            | "atmos"
            | "10bit"
            | "8bit"
            | "nf"
    ) || token.starts_with("ddp")
        || token.starts_with("aac")
}

fn is_language_or_subtitle_tag(token: &str) -> bool {
    matches!(
        token,
        "ja" | "jpn"
            | "jp"
            | "japanese"
            | "en"
            | "eng"
            | "english"
            | "forced"
            | "sdh"
            | "signs"
            | "dialogue"
            | "subtitle"
            | "subtitles"
    )
}

fn update_from_scan(item: &mut StoredPlaylistItem, scanned: &ScannedVideo) -> bool {
    let changed = item.path != scanned.path
        || item.display_name != scanned.display_name
        || item.size_bytes != scanned.size_bytes
        || item.modified_unix_nanos != scanned.modified_unix_nanos
        || item.automatic_japanese_subtitle != scanned.automatic_japanese_subtitle
        || item.automatic_translation_subtitle != scanned.automatic_translation_subtitle;
    item.path.clone_from(&scanned.path);
    item.display_name.clone_from(&scanned.display_name);
    item.size_bytes = scanned.size_bytes;
    item.modified_unix_nanos = scanned.modified_unix_nanos;
    item.automatic_japanese_subtitle
        .clone_from(&scanned.automatic_japanese_subtitle);
    item.automatic_translation_subtitle
        .clone_from(&scanned.automatic_translation_subtitle);
    changed
}

fn public_playlist(playlist: &StoredPlaylist) -> PlaylistV1 {
    PlaylistV1 {
        playlist_id: playlist.playlist_id.clone(),
        display_name: playlist.display_name.clone(),
        revision: playlist.revision,
        scan_revision: playlist.scan_revision,
        change_id: playlist.change_id.clone(),
        items: playlist
            .items
            .iter()
            .enumerate()
            .map(|(ordinal, item)| PlaylistItemV1 {
                item_id: item.item_id.clone(),
                display_name: item.display_name.clone(),
                ordinal,
                japanese_subtitle: public_subtitle(
                    item.japanese_subtitle_override.as_ref(),
                    &item.automatic_japanese_subtitle,
                ),
                translation_subtitle: public_subtitle(
                    item.translation_subtitle_override.as_ref(),
                    &item.automatic_translation_subtitle,
                ),
            })
            .collect(),
        current_item_id: playlist.current_item_id.clone(),
        watch_state: PlaylistWatchStateV1 {
            status: if playlist.directory.is_dir() {
                PlaylistWatchStatusV1::Watching
            } else {
                PlaylistWatchStatusV1::Unavailable
            },
            last_scan_unix_ms: playlist.last_scan_unix_ms,
        },
    }
}

fn public_subtitle(
    override_path: Option<&PathBuf>,
    automatic: &StoredSubtitleMatch,
) -> PlaylistSubtitleV1 {
    if let Some(path) = override_path.and_then(|path| validate_subtitle_path(path).ok()) {
        return PlaylistSubtitleV1 {
            kind: PlaylistSubtitleKindV1::Override,
            display_name: Some(safe_display_name(&path, "Subtitle file")),
            confidence: Some(100),
            reason: Some("Manual override".into()),
        };
    }
    match automatic {
        StoredSubtitleMatch::Matched {
            path,
            confidence,
            reason,
        } => match validate_subtitle_path(path) {
            Ok(path) => PlaylistSubtitleV1 {
                kind: PlaylistSubtitleKindV1::Auto,
                display_name: Some(safe_display_name(&path, "Subtitle file")),
                confidence: Some(*confidence),
                reason: Some(reason.clone()),
            },
            Err(_) => PlaylistSubtitleV1 {
                kind: PlaylistSubtitleKindV1::None,
                display_name: None,
                confidence: None,
                reason: Some("The matched subtitle is no longer a safe readable file.".into()),
            },
        },
        StoredSubtitleMatch::Ambiguous { confidence, reason } => PlaylistSubtitleV1 {
            kind: PlaylistSubtitleKindV1::Ambiguous,
            display_name: None,
            confidence: Some(*confidence),
            reason: Some(reason.clone()),
        },
        StoredSubtitleMatch::None => PlaylistSubtitleV1 {
            kind: PlaylistSubtitleKindV1::None,
            display_name: None,
            confidence: None,
            reason: Some("No confident subtitle match was found.".into()),
        },
    }
}

fn canonical_folder(path: &Path) -> Result<PathBuf, AppErrorV1> {
    if fs::symlink_metadata(path)
        .map_err(folder_read_error)?
        .file_type()
        .is_symlink()
    {
        return Err(invalid_request(
            "Choose a local folder directly rather than through a symbolic link.",
        ));
    }
    let canonical = fs::canonicalize(path).map_err(folder_read_error)?;
    if !canonical.is_dir() {
        return Err(invalid_request("Choose a folder containing local videos."));
    }
    Ok(canonical)
}

fn canonical_child(directory: &Path, path: &Path) -> Result<PathBuf, AppErrorV1> {
    let canonical = fs::canonicalize(path).map_err(folder_read_error)?;
    if !canonical.starts_with(directory) {
        return Err(invalid_request(
            "A folder entry resolved outside the selected folder.",
        ));
    }
    Ok(canonical)
}

fn validate_playlist_media_path(directory: &Path, path: &Path) -> Result<(), AppErrorV1> {
    let canonical_directory = canonical_folder(directory)?;
    let original_metadata = fs::symlink_metadata(path).map_err(folder_read_error)?;
    if original_metadata.file_type().is_symlink() || !original_metadata.is_file() {
        return Err(stale_error(
            "The selected playlist item changed or is no longer a regular local video.",
        ));
    }
    let canonical_path = canonical_child(&canonical_directory, path)?;
    if !is_video_path(&canonical_path) {
        return Err(stale_error(
            "The selected playlist item changed or is no longer a supported video.",
        ));
    }
    Ok(())
}

fn validate_subtitle_path(path: &Path) -> Result<PathBuf, AppErrorV1> {
    if fs::symlink_metadata(path)
        .map_err(folder_read_error)?
        .file_type()
        .is_symlink()
    {
        return Err(invalid_request(
            "Choose a subtitle file directly rather than through a symbolic link.",
        ));
    }
    let canonical = fs::canonicalize(path).map_err(folder_read_error)?;
    let metadata = canonical.metadata().map_err(folder_read_error)?;
    if !metadata.is_file() || !is_subtitle_path(&canonical) {
        return Err(invalid_request(
            "Choose an SRT, ASS, SSA, or WebVTT subtitle file.",
        ));
    }
    Ok(canonical)
}

fn valid_subtitle_choice(path: Option<&PathBuf>) -> Option<PathBuf> {
    path.and_then(|path| validate_subtitle_path(path).ok())
}

fn resolved_subtitle_choice(
    override_path: Option<&PathBuf>,
    automatic: &StoredSubtitleMatch,
) -> Option<ResolvedPlaylistSubtitle> {
    if let Some(path) = valid_subtitle_choice(override_path) {
        return Some(ResolvedPlaylistSubtitle {
            path,
            origin: SubtitleOriginV1::SelectedFile,
        });
    }
    let StoredSubtitleMatch::Matched { path, .. } = automatic else {
        return None;
    };
    valid_subtitle_choice(Some(path)).map(|path| ResolvedPlaylistSubtitle {
        path,
        origin: SubtitleOriginV1::AdjacentFile,
    })
}

fn validate_stored_playlist(playlist: &StoredPlaylist) -> Result<(), AppErrorV1> {
    validate_id(&playlist.playlist_id)?;
    validate_id(&playlist.change_id)?;
    if playlist.display_name.is_empty()
        || playlist.display_name.len() > 1_024
        || playlist.items.len() > MAX_PLAYLIST_ITEMS
        || playlist.ignored_item_ids.len() > MAX_PLAYLIST_ITEMS
    {
        return Err(storage_error("The saved playlist metadata was invalid."));
    }
    let mut item_ids = BTreeSet::new();
    for item in &playlist.items {
        validate_id(&item.item_id)?;
        if !item_ids.insert(&item.item_id) {
            return Err(storage_error(
                "The saved playlist contained duplicate items.",
            ));
        }
    }
    if playlist
        .current_item_id
        .as_ref()
        .is_some_and(|current| !item_ids.contains(current))
    {
        return Err(storage_error(
            "The saved playlist selected an unavailable item.",
        ));
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), AppErrorV1> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid_request("The playlist selection was invalid."));
    }
    Ok(())
}

fn is_video_path(path: &Path) -> bool {
    extension_is(path, &["mp4", "m4v", "webm", "mkv", "mov", "avi"])
}

fn is_subtitle_path(path: &Path) -> bool {
    extension_is(path, &["srt", "ass", "ssa", "vtt"])
}

fn extension_is(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn directory_change_id(videos: &[ScannedVideo], subtitles: &[PathBuf]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"migaku-playlist-directory-change-v1\0");
    for video in videos {
        update_path_hash(&mut hasher, &video.path);
        hasher.update(video.size_bytes.to_le_bytes());
        hasher.update(video.modified_unix_nanos.to_le_bytes());
    }
    for subtitle in subtitles {
        update_path_hash(&mut hasher, subtitle);
        if let Ok(metadata) = subtitle.metadata() {
            hasher.update(metadata.len().to_le_bytes());
            hasher.update(modified_unix_nanos(&metadata).to_le_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

fn stable_id(prefix: &[u8], path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    update_path_hash(&mut hasher, path);
    hex::encode(hasher.finalize())
}

#[cfg(windows)]
fn update_path_hash(hasher: &mut Sha256, path: &Path) {
    use std::os::windows::ffi::OsStrExt;

    for value in path.as_os_str().encode_wide() {
        hasher.update(value.to_le_bytes());
    }
}

#[cfg(not(windows))]
fn update_path_hash(hasher: &mut Sha256, path: &Path) {
    use std::os::unix::ffi::OsStrExt;

    hasher.update(path.as_os_str().as_bytes());
}

fn modified_unix_nanos(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn natural_path_cmp(left: &Path, right: &Path) -> Ordering {
    natural_cmp(
        &left
            .file_name()
            .unwrap_or(left.as_os_str())
            .to_string_lossy(),
        &right
            .file_name()
            .unwrap_or(right.as_os_str())
            .to_string_lossy(),
    )
}

fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    loop {
        match (left.peek(), right.peek()) {
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let left_digits = take_digits(&mut left);
                let right_digits = take_digits(&mut right);
                let ordering = left_digits
                    .trim_start_matches('0')
                    .len()
                    .cmp(&right_digits.trim_start_matches('0').len())
                    .then_with(|| {
                        left_digits
                            .trim_start_matches('0')
                            .cmp(right_digits.trim_start_matches('0'))
                    })
                    .then_with(|| left_digits.len().cmp(&right_digits.len()));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(_), Some(_)) => {
                let left_char = left.next().map(|value| value.to_ascii_lowercase());
                let right_char = right.next().map(|value| value.to_ascii_lowercase());
                let ordering = left_char.cmp(&right_char);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut digits = String::new();
    while chars.peek().is_some_and(char::is_ascii_digit) {
        if let Some(value) = chars.next() {
            digits.push(value);
        }
    }
    digits
}

fn safe_display_name(path: &Path, fallback: &str) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn no_playlist_error() -> AppErrorV1 {
    invalid_request("Choose a video folder before using the playlist.")
}

fn stale_error(message: &str) -> AppErrorV1 {
    AppErrorV1::new(error_codes::STALE_REVISION, message, true)
}

fn invalid_request(message: &str) -> AppErrorV1 {
    AppErrorV1::new(error_codes::INVALID_REQUEST, message, false)
}

fn folder_read_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_SCOPE_DENIED,
        "The selected video folder could not be read. Locate it again or check its permissions.",
        true,
    )
    .with_diagnostics(error.to_string())
}

fn storage_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::STORAGE_FAILED,
        "The playlist could not be saved safely.",
        true,
    )
    .with_diagnostics(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path) -> Result<(), std::io::Error> {
        fs::write(path, b"fixture")
    }

    #[test]
    fn folder_scan_is_bounded_non_recursive_and_naturally_sorted()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        write(&directory.path().join("Episode 10.mkv"))?;
        write(&directory.path().join("Episode 2.MP4"))?;
        write(&directory.path().join("notes.txt"))?;
        fs::create_dir(directory.path().join("nested"))?;
        write(&directory.path().join("nested").join("Episode 1.mkv"))?;
        let manager = PlaylistManager::load(Arc::new(Storage::in_memory()?))?;
        let playlist = manager
            .import_folder(directory.path(), PlaylistScanModeV1::AutoImport)?
            .playlist;
        assert_eq!(
            playlist
                .items
                .iter()
                .map(|item| item.display_name.as_str())
                .collect::<Vec<_>>(),
            ["Episode 2.MP4", "Episode 10.mkv"]
        );
        assert!(
            playlist
                .items
                .iter()
                .all(|item| !item.item_id.contains("Episode"))
        );
        Ok(())
    }

    #[test]
    fn preview_stages_new_files_and_auto_import_preserves_manual_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        write(&directory.path().join("Episode 1.mkv"))?;
        write(&directory.path().join("Episode 2.mkv"))?;
        let manager = PlaylistManager::load(Arc::new(Storage::in_memory()?))?;
        let playlist = manager
            .import_folder(directory.path(), PlaylistScanModeV1::AutoImport)?
            .playlist;
        let second = playlist.items[1].item_id.clone();
        manager.reorder_item(&second, 0)?;
        write(&directory.path().join("Episode 3.mkv"))?;

        let preview = manager.rescan(PlaylistScanModeV1::Preview)?;
        assert_eq!(preview.playlist.items.len(), 2);
        assert_eq!(preview.pending_items.len(), 1);
        assert_eq!(preview.playlist.items[0].item_id, second);

        let imported = manager.import_discoveries(&[preview.pending_items[0].item_id.clone()])?;
        assert_eq!(imported.items.len(), 3);
        assert_eq!(imported.items[0].item_id, second);
        assert_eq!(imported.items[2].display_name, "Episode 3.mkv");
        Ok(())
    }

    #[test]
    fn removed_items_stay_ignored_during_auto_import() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        write(&directory.path().join("Episode 1.mkv"))?;
        let manager = PlaylistManager::load(Arc::new(Storage::in_memory()?))?;
        let playlist = manager
            .import_folder(directory.path(), PlaylistScanModeV1::AutoImport)?
            .playlist;
        let item_id = playlist.items[0].item_id.clone();
        assert!(manager.remove_item(&item_id)?.items.is_empty());
        assert!(
            manager
                .rescan(PlaylistScanModeV1::AutoImport)?
                .playlist
                .items
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn japanese_and_translation_matches_and_overrides_persist()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        write(&directory.path().join("Show.mkv"))?;
        write(&directory.path().join("Show.ja.ass"))?;
        write(&directory.path().join("Show.en.srt"))?;
        let override_path = directory.path().join("manual.vtt");
        write(&override_path)?;
        let storage = Arc::new(Storage::in_memory()?);
        let manager = PlaylistManager::load(storage.clone())?;
        let playlist = manager
            .import_folder(directory.path(), PlaylistScanModeV1::AutoImport)?
            .playlist;
        let item_id = playlist.items[0].item_id.clone();
        assert_eq!(
            playlist.items[0].japanese_subtitle.kind,
            PlaylistSubtitleKindV1::Auto
        );
        assert_eq!(
            playlist.items[0].translation_subtitle.kind,
            PlaylistSubtitleKindV1::Auto
        );
        let canonical_override = manager.canonical_subtitle_override(&override_path)?;
        manager.set_subtitle_override(
            &item_id,
            SubtitleRoleV1::Japanese,
            Some(canonical_override),
        )?;

        drop(manager);
        let restored = PlaylistManager::load(storage)?
            .current()
            .ok_or("missing restored playlist")?;
        assert_eq!(
            restored.items[0].japanese_subtitle.kind,
            PlaylistSubtitleKindV1::Override
        );
        assert_eq!(
            restored.items[0].japanese_subtitle.display_name.as_deref(),
            Some("manual.vtt")
        );
        Ok(())
    }

    #[test]
    fn ambiguous_same_language_subtitles_require_an_override()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        write(&directory.path().join("Show.mkv"))?;
        write(&directory.path().join("Show.ja.ass"))?;
        write(&directory.path().join("Show.ja.srt"))?;
        let manager = PlaylistManager::load(Arc::new(Storage::in_memory()?))?;
        let playlist = manager
            .import_folder(directory.path(), PlaylistScanModeV1::AutoImport)?
            .playlist;
        assert_eq!(
            playlist.items[0].japanese_subtitle.kind,
            PlaylistSubtitleKindV1::Ambiguous
        );
        Ok(())
    }

    #[test]
    fn preview_folder_import_stages_every_video_without_adding_items()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        write(&directory.path().join("Episode 1.mkv"))?;
        write(&directory.path().join("Episode 2.mkv"))?;
        let manager = PlaylistManager::load(Arc::new(Storage::in_memory()?))?;
        let result = manager.import_folder(directory.path(), PlaylistScanModeV1::Preview)?;
        assert!(result.playlist.items.is_empty());
        assert_eq!(result.pending_items.len(), 2);
        Ok(())
    }

    #[test]
    fn normalized_release_titles_match_but_different_episodes_never_pair()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        write(
            &directory
                .path()
                .join("Soratobu.Kouhoushitsu.EP01.1080p.NF.WEB-DL.DDP2.0.H.264-MagicStar.mkv"),
        )?;
        write(
            &directory
                .path()
                .join("Soratobu Kouhoushitsu - EP01 [Fansub].ja.ass"),
        )?;
        write(&directory.path().join("Soratobu Kouhoushitsu - EP02.ja.srt"))?;
        write(&directory.path().join("Soratobu Kouhoushitsu - EP02.en.srt"))?;
        let manager = PlaylistManager::load(Arc::new(Storage::in_memory()?))?;
        let playlist = manager
            .import_folder(directory.path(), PlaylistScanModeV1::AutoImport)?
            .playlist;
        assert_eq!(
            playlist.items[0].japanese_subtitle.display_name.as_deref(),
            Some("Soratobu Kouhoushitsu - EP01 [Fansub].ja.ass")
        );
        assert_eq!(
            playlist.items[0].translation_subtitle.kind,
            PlaylistSubtitleKindV1::None
        );
        assert!(
            playlist.items[0]
                .japanese_subtitle
                .confidence
                .is_some_and(|confidence| confidence >= 90)
        );
        Ok(())
    }

    #[test]
    fn replaced_override_is_not_returned_as_a_safe_subtitle()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let media = directory.path().join("Show.mkv");
        let override_path = directory.path().join("manual.srt");
        write(&media)?;
        write(&override_path)?;
        let manager = PlaylistManager::load(Arc::new(Storage::in_memory()?))?;
        let playlist = manager
            .import_folder(directory.path(), PlaylistScanModeV1::AutoImport)?
            .playlist;
        let item_id = playlist.items[0].item_id.clone();
        manager.set_subtitle_override(
            &item_id,
            SubtitleRoleV1::Japanese,
            Some(manager.canonical_subtitle_override(&override_path)?),
        )?;
        fs::remove_file(&override_path)?;
        fs::create_dir(&override_path)?;
        let resolved = manager.resolve_item(&item_id)?;
        assert!(resolved.japanese_subtitle.is_none());
        let public = manager.current().ok_or("missing playlist")?;
        assert_eq!(
            public.items[0].japanese_subtitle.kind,
            PlaylistSubtitleKindV1::None
        );
        Ok(())
    }

    #[test]
    fn symlink_replacement_is_rejected_when_platform_permissions_allow_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let media = directory.path().join("Show.mkv");
        let override_path = directory.path().join("manual.srt");
        let target = directory.path().join("target.srt");
        write(&media)?;
        write(&override_path)?;
        write(&target)?;
        let manager = PlaylistManager::load(Arc::new(Storage::in_memory()?))?;
        let playlist = manager
            .import_folder(directory.path(), PlaylistScanModeV1::AutoImport)?
            .playlist;
        let item_id = playlist.items[0].item_id.clone();
        manager.set_subtitle_override(
            &item_id,
            SubtitleRoleV1::Japanese,
            Some(manager.canonical_subtitle_override(&override_path)?),
        )?;
        fs::remove_file(&override_path)?;
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&target, &override_path).is_err() {
            return Ok(());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &override_path)?;
        assert!(manager.resolve_item(&item_id)?.japanese_subtitle.is_none());
        Ok(())
    }
}
