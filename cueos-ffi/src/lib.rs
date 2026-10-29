//! C FFI surface over dj-usb-tkit's headless `BackendCommands` layer.
//!
//! Contract: every call returns a JSON `ApiResponse` string
//! (`{"ok":bool,"data":...,"error":...}`), allocated by Rust and freed by
//! the caller through `cueos_string_free`. `cueos_command` dispatches by
//! command name; payloads are JSON objects matching the typed request
//! structs in `backend/src/models.rs` (camelCase, same wire format the
//! Tauri frontend uses).
use serde::de::DeserializeOwned;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char};
use std::sync::{Mutex, OnceLock};

use backend::commands::BackendCommands;
use backend::models::*;

static ENGINE: OnceLock<Mutex<Option<BackendCommands>>> = OnceLock::new();

fn engine() -> &'static Mutex<Option<BackendCommands>> {
    ENGINE.get_or_init(|| Mutex::new(None))
}

fn err_json(msg: &str) -> String {
    serde_json::json!({"ok": false, "error": {"code": "ffi-error", "message": msg}}).to_string()
}

fn ser<T: serde::Serialize>(r: ApiResponse<T>) -> String {
    serde_json::to_string(&r).unwrap_or_else(|e| err_json(&format!("serialize: {e}")))
}

fn parse_req<T: DeserializeOwned>(payload: Option<&str>) -> Result<T, String> {
    let raw = payload.unwrap_or("null");
    serde_json::from_str(raw).map_err(|e| format!("bad payload: {e}"))
}

fn ptr_to_payload(p: *const c_char) -> Result<Option<String>, String> {
    if p.is_null() {
        return Ok(None);
    }
    let s = unsafe { CStr::from_ptr(p) }
        .to_str()
        .map_err(|e| format!("payload is not UTF-8: {e}"))?
        .to_string();
    Ok(Some(s))
}

macro_rules! with_req {
    ($eng:expr, $ty:ty, $payload:expr, $m:ident) => {{
        match parse_req::<$ty>($payload.as_deref()) {
            Ok(req) => ser($eng.$m(req)),
            Err(e) => err_json(&e),
        }
    }};
}

macro_rules! no_req {
    ($eng:expr, $m:ident) => {
        ser($eng.$m())
    };
}

/// For commands that only exist in a `*_with_progress` variant: run them
/// with a no-op progress closure (F: FnMut(usize, usize, &str)).
macro_rules! with_progress_req {
    ($eng:expr, $ty:ty, $payload:expr, $m:ident) => {{
        match parse_req::<$ty>($payload.as_deref()) {
            Ok(req) => ser($eng.$m(req, |_, _, _| {})),
            Err(e) => err_json(&e),
        }
    }};
}

/// Commands taking a bare `bool` argument instead of a request struct.
macro_rules! bool_arg {
    ($eng:expr, $payload:expr, $key:literal, $m:ident) => {{
        let v = $payload.as_deref()
            .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
            .and_then(|v| v.get($key).and_then(|b| b.as_bool()))
            .unwrap_or(false);
        ser($eng.$m(v))
    }};
}

/// Initialize the engine with a data directory. Re-initializing replaces the
/// previous engine (its DB pool is dropped first).
#[unsafe(no_mangle)]
pub extern "C" fn cueos_engine_init(data_dir: *const c_char) -> *mut c_char {
    let dir = match ptr_to_payload(data_dir) {
        Ok(Some(s)) => s,
        Ok(None) => return ret(&err_json("cueos_engine_init: data_dir is NULL")),
        Err(e) => return ret(&err_json(&e)),
    };
    match BackendCommands::new(&dir) {
        Ok(cmds) => {
            let mut slot = engine()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *slot = Some(cmds);
            ret(&serde_json::json!({"ok": true, "data": {"dataDir": dir}}).to_string())
        }
        Err(e) => ret(&serde_json::to_string(&e)
            .unwrap_or_else(|_| err_json("engine init failed"))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cueos_command(name: *const c_char, payload: *const c_char) -> *mut c_char {
    let cmd = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return ret(&err_json("command name is not UTF-8")),
    };
    let payload = match ptr_to_payload(payload) {
        Ok(p) => p,
        Err(e) => return ret(&err_json(&e)),
    };
    let guard = engine().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(eng) = guard.as_ref() else {
        return ret(&err_json("engine not initialized: call cueos_engine_init first"));
    };

    let out = match cmd.as_str() {
        // ── Library ─────────────────────────────────────────────
        "scan_library" => with_req!(eng, ScanLibraryRequest, payload, scan_library),
        "scan_rekordbox_db" => with_req!(eng, ScanRekordboxDbRequest, payload, scan_rekordbox_db),
        "plan_rekordbox_relocation" => with_req!(eng, PlanRekordboxRelocationRequest, payload, plan_rekordbox_relocation),
        "scan_mixxx_db" => with_req!(eng, ScanMixxxDbRequest, payload, scan_mixxx_db),
        "list_rekordbox_playlists" => with_req!(eng, ListExternalPlaylistsRequest, payload, list_rekordbox_playlists),
        "import_rekordbox_playlist" => with_req!(eng, ImportExternalPlaylistRequest, payload, import_rekordbox_playlist),
        "list_mixxx_playlists" => with_req!(eng, ListExternalPlaylistsRequest, payload, list_mixxx_playlists),
        "import_mixxx_playlist" => with_req!(eng, ImportExternalPlaylistRequest, payload, import_mixxx_playlist),
        "detect_external_rekordbox_db" => no_req!(eng, detect_external_rekordbox_db),
        "detect_external_mixxx_db" => no_req!(eng, detect_external_mixxx_db),
        "browse_source_files" => with_req!(eng, BrowseSourceFilesRequest, payload, browse_source_files),
        "check_source_roots" => with_req!(eng, CheckSourceRootsRequest, payload, check_source_roots),
        "get_source_root_analysis" => with_req!(eng, GetSourceRootAnalysisRequest, payload, get_source_root_analysis),
        "materialize_source_track" => with_req!(eng, MaterializeSourceTrackRequest, payload, materialize_source_track),
        "resolve_track_identity" => with_req!(eng, ResolveTrackIdentityRequest, payload, resolve_track_identity),
        "remove_tracks_by_source_roots" => with_req!(eng, RemoveTracksBySourceRootsRequest, payload, remove_tracks_by_source_roots),
        "get_track_detail" => with_req!(eng, GetTrackDetailRequest, payload, get_track_detail),
        "save_track_analysis_edits" => with_req!(eng, SaveTrackAnalysisEditsRequest, payload, save_track_analysis_edits),
        "save_track_metadata_edits" => with_req!(eng, SaveTrackMetadataEditsRequest, payload, save_track_metadata_edits),
        "get_tracks_metadata_summary" => with_req!(eng, GetTracksMetadataSummaryRequest, payload, get_tracks_metadata_summary),
        "get_usb_tracks_metadata_summary" => with_req!(eng, GetUsbTracksMetadataSummaryRequest, payload, get_usb_tracks_metadata_summary),
        "save_usb_track_metadata_edits" => with_progress_req!(eng, SaveUsbTrackMetadataEditsRequest, payload, save_usb_track_metadata_edits_with_progress),
        "get_usb_track_detail" => with_req!(eng, GetUsbTrackDetailRequest, payload, get_usb_track_detail),
        "save_usb_track_analysis_edits" => with_req!(eng, SaveUsbTrackAnalysisEditsRequest, payload, save_usb_track_analysis_edits),
        "relocate_source_root" => with_req!(eng, RelocateSourceRootRequest, payload, relocate_source_root),
        "get_tracks_by_ids_with_previews" => with_req!(eng, GetTracksByIdsRequest, payload, get_tracks_by_ids_with_previews),
        "resolve_playback_source" => with_req!(eng, ResolvePlaybackSourceRequest, payload, resolve_playback_source),
        "forget_source_folder_listings" => { eng.forget_source_folder_listings(); serde_json::json!({"ok": true}).to_string() },

        // ── Playlists ───────────────────────────────────────────
        "create_playlist" => with_req!(eng, CreatePlaylistRequest, payload, create_playlist),
        "rename_playlist" => with_req!(eng, RenamePlaylistRequest, payload, rename_playlist),
        "reorder_playlists" => with_req!(eng, ReorderPlaylistsRequest, payload, reorder_playlists),
        "delete_playlist" => with_req!(eng, DeletePlaylistRequest, payload, delete_playlist),
        "list_playlists" => no_req!(eng, list_playlists),
        "get_playlist_tracks" => with_req!(eng, GetPlaylistTracksRequest, payload, get_playlist_tracks),
        "get_playlist_export_space" => with_req!(eng, GetPlaylistExportSpaceRequest, payload, get_playlist_export_space),
        "add_track_candidates_to_playlist" => with_req!(eng, AddTrackCandidatesToPlaylistRequest, payload, add_track_candidates_to_playlist),
        "list_matching_track_ids" => with_req!(eng, ListMatchingTrackIdsRequest, payload, list_matching_track_ids),
        "add_library_selection_to_playlist" => with_req!(eng, AddLibrarySelectionToPlaylistRequest, payload, add_library_selection_to_playlist),
        "add_library_selection_to_playlists" => with_req!(eng, AddLibrarySelectionToPlaylistsRequest, payload, add_library_selection_to_playlists),
        "add_track_candidates_to_playlists" => with_req!(eng, AddTrackCandidatesToPlaylistsRequest, payload, add_track_candidates_to_playlists),
        "get_playlist_membership" => with_req!(eng, GetPlaylistMembershipRequest, payload, get_playlist_membership),
        "remove_tracks_from_playlist" => with_req!(eng, RemoveTracksFromPlaylistRequest, payload, remove_tracks_from_playlist),
        "reorder_playlist_tracks" => with_req!(eng, ReorderPlaylistTracksRequest, payload, reorder_playlist_tracks),
        "refresh_playlist_export_status" => with_req!(eng, RefreshPlaylistExportStatusRequest, payload, refresh_playlist_export_status),

        // ── Settings ────────────────────────────────────────────
        "get_frontend_settings" => no_req!(eng, get_frontend_settings),
        "set_frontend_setting" => with_req!(eng, SetFrontendSettingRequest, payload, set_frontend_setting),

        // ── USB devices / backups ───────────────────────────────
        "validate_usb_root" => with_req!(eng, ValidateUsbRootRequest, payload, validate_usb_root),
        "list_usb_devices" => no_req!(eng, list_usb_devices),
        "prune_usb_device" => with_req!(eng, PruneUsbDeviceRequest, payload, prune_usb_device),
        "get_usb_device_name" => with_req!(eng, GetUsbDeviceNameRequest, payload, get_usb_device_name),
        "set_usb_device_name" => with_req!(eng, SetUsbDeviceNameRequest, payload, set_usb_device_name),
        "list_usb_backups" => with_req!(eng, ListUsbBackupsRequest, payload, list_usb_backups),
        "restore_usb_backup" => with_req!(eng, RestoreUsbBackupRequest, payload, restore_usb_backup),
        "delete_usb_backup" => with_req!(eng, DeleteUsbBackupRequest, payload, delete_usb_backup),
        "pending_data_migrations" => no_req!(eng, pending_data_migrations),
        "run_data_migrations" => ser(eng.run_data_migrations_with_progress(|_, _, _| {})),

        // ── USB content ─────────────────────────────────────────
        "initialize_usb" => with_req!(eng, InitializeUsbRequest, payload, initialize_usb),
        "fetch_usb_playlists" => with_req!(eng, FetchUsbPlaylistsRequest, payload, fetch_usb_playlists),
        "fetch_usb_histories" => with_req!(eng, FetchUsbHistoriesRequest, payload, fetch_usb_histories),
        "fetch_usb_playlist_tracks" => with_req!(eng, FetchUsbTracksRequest, payload, fetch_usb_playlist_tracks),
        "fetch_usb_history_tracks" => with_req!(eng, FetchUsbTracksRequest, payload, fetch_usb_history_tracks),
        "get_usb_player_menu_config" => with_req!(eng, GetUsbPlayerMenuConfigRequest, payload, get_usb_player_menu_config),
        "update_usb_player_menu_config" => with_req!(eng, UpdateUsbPlayerMenuConfigRequest, payload, update_usb_player_menu_config),
        "sync_usb_player_menu_edb_to_pdb" => with_req!(eng, GetUsbPlayerMenuConfigRequest, payload, sync_usb_player_menu_edb_to_pdb),
        "remove_usb_playlist" => with_req!(eng, RemoveUsbPlaylistRequest, payload, remove_usb_playlist),
        "reorder_usb_playlists" => with_req!(eng, ReorderUsbPlaylistsRequest, payload, reorder_usb_playlists),
        "inspect_usb_track" => with_req!(eng, InspectUsbTrackRequest, payload, inspect_usb_track),

        // ── Analysis / export / diagnostics ────────────────────
        "analyze_new_tracks" => with_req!(eng, AnalyzeNewTracksRequest, payload, analyze_new_tracks),
        "set_analysis_paused" => bool_arg!(eng, payload, "paused", set_analysis_paused),
        "cancel_analysis" => no_req!(eng, cancel_analysis),
        "export_to_usb" => with_req!(eng, ExportToUsbRequest, payload, export_to_usb),
        "run_usb_diagnostics" => with_req!(eng, RunUsbDiagnosticsRequest, payload, run_usb_diagnostics),
        "run_usb_parity_report" => with_req!(eng, RunUsbParityReportRequest, payload, run_usb_parity_report),
        "repair_usb_diagnostics" => with_req!(eng, RepairUsbDiagnosticsRequest, payload, repair_usb_diagnostics),

        _ => err_json(&format!("unknown command: {cmd}")),
    };
    ret(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn cueos_string_free(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)) };
    }
}

fn ret(s: &str) -> *mut c_char {
    // CStr keeps the NUL terminator out of the string itself; CString::new
    // re-appends it for the FFI boundary.
    match CString::new(s.as_bytes()) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}
