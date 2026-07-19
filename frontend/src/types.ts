export interface Task {
  id: string
  title: string
  filename: string
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
  created_at: string
  updated_at: string
  started_at: string
  finished_at: string
  available_actions: string[]
  queue_position: number
}

export interface Settings {
  token?: string
  download_dir?: string
  default_concurrency?: number
  max_concurrent_tasks?: number
  default_user_agent?: string
  default_referer?: string
  default_origin?: string
  default_cookie?: string
}

export interface UserscriptStatus {
  detected: boolean
  seen_before: boolean
  version: string
  page_origin: string
  last_seen_at: string
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
}
