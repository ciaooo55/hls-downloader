export interface Task {
  id: string
  task_type: 'hls' | 'dash' | 'http' | 'torrent' | 'ftp' | 'sftp'
  request_method?: 'GET' | 'POST'
  source_page_url: string
  mime_type: string
  title: string
  filename: string
  download_dir: string
  url: string
  status: string
  stage: string
  last_log: string
  total_segments: number
  completed_segments: number
  failed_segments: number
  downloaded_bytes: number
  total_bytes: number
  speed_bytes_per_sec: number
  eta_seconds: number
  active_workers: number
  max_workers: number
  active_slots: number
  concurrency: number
  reconnect_count: number
  connection_status: string
  post_percent: number
  error_message: string
  error_code: string
  error_stage: string
  error_url: string
  error_hint: string
  http_status: number
  error_attempt: number
  output_path: string
  expected_checksum: string
  checksum_algorithm: string
  checksum_actual: string
  checksum_verified: boolean | null
  output_is_file: boolean
  output_missing?: boolean
  file_access_token?: string
  created_at: string
  updated_at: string
  started_at: string
  finished_at: string
  scheduled_start_at?: string
  scheduled_stop_at?: string
  completion_action?: 'none' | 'shutdown' | 'sleep' | 'hibernate'
  mirrors?: string[]
  av_scan?: { state?: string; engine?: string; detail?: string; exit_code?: number }
  mirror_status?: Array<{ url: string; final_url?: string; state: string; detail?: string; ranges?: boolean }>
  available_actions: string[]
  queue_position: number
  playable_segments: number
  playable_duration: number
  media_duration: number
  playback_ready: boolean
  is_live?: boolean
  speed_limit_kib?: number
  speed_history?: number[]
  speed_peak_bytes_per_sec?: number
  connection_parts?: Array<{ start: number; end: number; done: number; state: string }>
  progress_percent: number
  uploaded_bytes: number
  upload_speed_bytes_per_sec: number
  peer_count: number
  seed_count: number
}

export interface PlaybackStatus {
  ready: boolean
  mode: 'hls' | 'file'
  available_segments: number
  total_segments: number
  available_duration: number
  total_duration: number
  complete: boolean
}

export interface PlaybackSeek {
  time: number
  index: number
  segment_start: number
  segment_end: number
  total_duration: number
}

export interface PlaybackSession extends PlaybackStatus {
  session_id: string
  playback_token: string
}

export interface Settings {
  download_dir?: string
  temp_dir?: string
  default_concurrency?: number
  max_concurrent_tasks?: number
  default_user_agent?: string
  default_referer?: string
  default_origin?: string
  default_cookie?: string
  default_cookie_configured?: boolean
  http_chunk_size_mb?: number
  download_speed_limit_kib?: number
  speed_schedule_enabled?: boolean
  speed_schedule_start?: string
  speed_schedule_end?: string
  speed_schedule_limit_kib?: number
  effective_download_speed_limit_kib?: number
  bt_upload_limit_kib?: number
  bt_max_connections?: number
  bt_enable_dht?: boolean
  watch_torrents?: boolean
  watch_dir?: string
  browser_takeover_enabled?: boolean
  browser_takeover_min_mb?: number
  browser_category_dirs?: Record<string, string>
  auto_category_dirs?: boolean
  queue_auto_start_enabled?: boolean
  queue_auto_start_time?: string
  queue_auto_stop_enabled?: boolean
  queue_auto_stop_time?: string
  queue_active_days?: number[]
  live_record_max_minutes?: number
  download_subtitles?: boolean
  skip_ad_segments?: boolean
  clipboard_watch?: boolean
  completion_sound_enabled?: boolean
  download_progress_window_enabled?: boolean
  download_complete_popup_enabled?: boolean
  resume_interrupted_on_startup?: boolean
  auto_retry_failed_max?: number
  av_scan_enabled?: boolean
  av_scan_command?: string
  av_scan_fail_on_threat?: boolean
  existing_file_policy?: 'rename' | 'overwrite' | 'skip'
  tvbox_endpoint?: string
  cast_device?: { id: string; protocol: 'dlna' | 'chromecast'; location: string; control_url: string; service_type: string; label: string; host: string }
  site_profiles?: Array<{ host: string; enabled?: boolean; user_agent?: string; referer?: string; origin?: string; cookie?: string; download_dir?: string; request_headers?: Record<string, string>; concurrency?: number; speed_limit_kib?: number; proxy_mode?: '' | 'direct' | 'system' | 'manual'; proxy_url?: string }>
  proxy_mode?: 'system' | 'direct' | 'manual'
  proxy_url?: string
  proxy_url_configured?: boolean
  proxy_bypass?: string[]
}

export interface LegalStatus {
  accepted: boolean
  required_version: string
  document_digest: string
  accepted_version: string
  accepted_at: string
  record_location: 'local_config'
}

export interface LegalDocument extends LegalStatus {
  title: string
  content: string
  privacy_document: string
  privacy_content: string
}

export interface LegalAcceptanceInput {
  version: string
  document_digest: string
  accepted: boolean
}

export interface TorrentFileEntry {
  index: number
  path: string
  size: number
  offset?: number
}

export interface BrowserStatus {
  detected: boolean
  seen_before: boolean
  version?: string
  state?: 'connected' | 'inactive' | 'not_detected'
  message?: string
  desktop_version?: string
  recommended_version?: string
  minimum_version?: string
  release_url?: string
  needs_upgrade?: boolean
  clients?: Array<{
    id: string
    browser: 'edge' | 'chrome' | 'chromium' | 'brave' | 'vivaldi' | 'opera' | 'firefox' | 'unknown'
    version?: string
    last_seen: number
    active: boolean
    needs_upgrade: boolean
  }>
  active_versions?: string[]
  client_count?: number
}

export interface UpdateInfo {
  current_version: string
  latest_version: string
  available: boolean
  can_auto_install: boolean
  release_url: string
  download_url: string
  size: number
  digest: string
  notes: string
  download_directory: string
  asset_kind: 'installer' | 'portable'
}
