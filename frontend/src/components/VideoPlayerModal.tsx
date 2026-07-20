import Hls from 'hls.js/light'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Activity,
  Gauge,
  LoaderCircle,
  Maximize,
  Minimize,
  Pause,
  PictureInPicture2,
  Play,
  RotateCcw,
  RotateCw,
  Volume2,
  VolumeX,
  X,
} from 'lucide-react'

import {
  closePlaybackSession,
  createPlaybackSession,
  fetchPlaybackStatus,
  getToken,
  heartbeatPlayback,
  playbackMediaUrl,
  playbackPlaylistUrl,
  requestPlaybackSeek,
} from '../api'
import { fmtSpeed } from '../format'
import { isRunningStatus } from '../taskState'
import {
  PLAYBACK_RATES,
  effectivePlaybackDuration,
  formatPlayerTime,
  isTimeSeekable,
  thumbnailBucket,
  thumbnailLeft,
  timelineTime,
} from '../playerModel'
import { statusLabel } from '../taskPresentation'
import type { PlaybackSession, PlaybackStatus, Task } from '../types'


const THUMBNAIL_CACHE_LIMIT = 48


function waitForMediaEvent(
  media: HTMLVideoElement,
  eventName: 'loadedmetadata' | 'loadeddata' | 'seeked',
  timeout = 5000,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => finish(new Error('媒体读取超时')), timeout)
    const onEvent = () => finish()
    const onError = () => finish(new Error('媒体帧无法解码'))
    const finish = (error?: Error) => {
      window.clearTimeout(timer)
      media.removeEventListener(eventName, onEvent)
      media.removeEventListener('error', onError)
      if (error) reject(error); else resolve()
    }
    media.addEventListener(eventName, onEvent, { once: true })
    media.addEventListener('error', onError, { once: true })
  })
}


export default function VideoPlayerModal({ task, onClose }: {
  task: Task
  onClose: () => void
}) {
  const containerRef = useRef<HTMLElement | null>(null)
  const videoRef = useRef<HTMLVideoElement | null>(null)
  const timelineRef = useRef<HTMLDivElement | null>(null)
  const mainHlsRef = useRef<Hls | null>(null)
  const thumbnailVideoRef = useRef<HTMLVideoElement | null>(null)
  const thumbnailHlsRef = useRef<Hls | null>(null)
  const thumbnailSourceRef = useRef('')
  const thumbnailReadyRef = useRef<Promise<void> | null>(null)
  const thumbnailCacheRef = useRef(new Map<number, string>())
  const thumbnailTimerRef = useRef<number | null>(null)
  const thumbnailIdleRef = useRef<number | null>(null)
  const thumbnailBusyRef = useRef(false)
  const pendingThumbnailRef = useRef<{ time: number; key: number } | null>(null)
  const seekTimerRef = useRef<number | null>(null)
  const seekRequestRef = useRef(0)
  const resumePositionRef = useRef(0)
  const mediaErrorRecoveries = useRef(0)

  const [session, setSession] = useState<PlaybackSession | null>(null)
  const [playbackStatus, setPlaybackStatus] = useState<PlaybackStatus | null>(null)
  const [mode, setMode] = useState<'hls' | 'file'>('hls')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [paused, setPaused] = useState(true)
  const [currentTime, setCurrentTime] = useState(0)
  const [mediaDuration, setMediaDuration] = useState(0)
  const [bufferedEnd, setBufferedEnd] = useState(0)
  const [muted, setMuted] = useState(false)
  const [volume, setVolume] = useState(1)
  const [rate, setRate] = useState(1)
  const [fullscreen, setFullscreen] = useState(false)
  const [hover, setHover] = useState<{
    time: number
    left: number
    key: number
    image: string
  } | null>(null)

  const effectiveDuration = useMemo(() => {
    return effectivePlaybackDuration(
      mode,
      playbackStatus?.total_duration || 0,
      task.media_duration || 0,
      playbackStatus?.available_duration || 0,
      task.playable_duration || 0,
      mediaDuration,
    )
  }, [mediaDuration, mode, playbackStatus?.available_duration, playbackStatus?.total_duration, task.media_duration, task.playable_duration])

  const destroyThumbnailDecoder = useCallback(() => {
    thumbnailHlsRef.current?.destroy()
    thumbnailHlsRef.current = null
    thumbnailReadyRef.current = null
    thumbnailSourceRef.current = ''
    const media = thumbnailVideoRef.current
    if (media) {
      media.pause()
      media.removeAttribute('src')
      media.load()
    }
  }, [])

  useEffect(() => {
    let canceled = false
    let openedSession = ''
    setLoading(true)
    setError('')
    createPlaybackSession(task.id)
      .then(result => {
        openedSession = result.session_id
        if (canceled) {
          void closePlaybackSession(task.id, result.session_id).catch(() => {})
          return
        }
        setSession(result)
        setPlaybackStatus(result)
        setMode(result.mode)
      })
      .catch(reason => {
        if (!canceled) {
          setError(reason?.message || '无法打开内置播放器')
          setLoading(false)
        }
      })
    return () => {
      canceled = true
      if (openedSession) void closePlaybackSession(task.id, openedSession).catch(() => {})
    }
  }, [task.id])

  useEffect(() => {
    if (!session) return
    const heartbeat = window.setInterval(() => {
      void heartbeatPlayback(task.id, session.session_id).catch(() => {})
    }, 25000)
    return () => window.clearInterval(heartbeat)
  }, [session, task.id])

  useEffect(() => {
    if (!session || mode !== 'hls') return
    let stopped = false
    const update = async () => {
      try {
        const next = await fetchPlaybackStatus(task.id, session.session_id)
        if (stopped) return
        setPlaybackStatus(next)
        if (next.mode === 'file') {
          resumePositionRef.current = videoRef.current?.currentTime || 0
          setMode('file')
        }
      } catch (reason: any) {
        if (!stopped && reason?.status === 410) setError(reason.message)
      }
    }
    void update()
    const timer = window.setInterval(update, 1200)
    return () => { stopped = true; window.clearInterval(timer) }
  }, [mode, session, task.id])

  useEffect(() => {
    if (session && task.status === 'done' && mode !== 'file') {
      resumePositionRef.current = videoRef.current?.currentTime || 0
      setMode('file')
    }
  }, [mode, session, task.status])

  useEffect(() => {
    const video = videoRef.current
    if (!video || !session) return
    setLoading(true)
    setError('')
    mediaErrorRecoveries.current = 0
    mainHlsRef.current?.destroy()
    mainHlsRef.current = null
    video.pause()
    video.removeAttribute('src')
    video.load()

    if (mode === 'file') {
      video.src = playbackMediaUrl(task.id, session.session_id)
      video.load()
    } else if (Hls.isSupported()) {
      const hls = new Hls({
        startPosition: Math.max(0, resumePositionRef.current),
        maxBufferLength: 18,
        maxMaxBufferLength: 36,
        backBufferLength: 24,
        maxBufferSize: 24 * 1024 * 1024,
        liveSyncDurationCount: 1,
        liveMaxLatencyDurationCount: 6,
        manifestLoadingMaxRetry: 20,
        manifestLoadingRetryDelay: 500,
        fragLoadingMaxRetry: 12,
        fragLoadingRetryDelay: 350,
        xhrSetup(xhr) {
          xhr.setRequestHeader('X-Token', getToken())
        },
      })
      mainHlsRef.current = hls
      hls.attachMedia(video)
      hls.on(Hls.Events.MEDIA_ATTACHED, () => {
        hls.loadSource(playbackPlaylistUrl(task.id, session.session_id, true))
      })
      hls.on(Hls.Events.MANIFEST_PARSED, () => {
        setLoading(false)
        void video.play().catch(() => setPaused(true))
      })
      hls.on(Hls.Events.ERROR, (_event, data) => {
        if (!data.fatal) return
        if (data.type === Hls.ErrorTypes.NETWORK_ERROR && task.status !== 'done') {
          window.setTimeout(() => hls.startLoad(video.currentTime), 600)
          return
        }
        if (data.type === Hls.ErrorTypes.MEDIA_ERROR && mediaErrorRecoveries.current < 2) {
          mediaErrorRecoveries.current += 1
          hls.recoverMediaError()
          return
        }
        setLoading(false)
        setError('当前视频编码无法由内置播放器解码，可使用系统播放器打开')
      })
    } else if (video.canPlayType('application/vnd.apple.mpegurl')) {
      video.src = playbackPlaylistUrl(task.id, session.session_id, true)
      video.load()
    } else {
      setLoading(false)
      setError('当前 WebView 不支持 HLS 播放')
    }

    return () => {
      seekRequestRef.current += 1
      if (mainHlsRef.current) {
        mainHlsRef.current.destroy()
        mainHlsRef.current = null
      }
      video.pause()
      video.removeAttribute('src')
      video.load()
    }
  }, [mode, session, task.id])

  useEffect(() => {
    const video = videoRef.current
    if (!video) return
    const updateDuration = () => {
      if (Number.isFinite(video.duration)) setMediaDuration(video.duration || 0)
    }
    const updateProgress = () => {
      setCurrentTime(video.currentTime || 0)
      resumePositionRef.current = video.currentTime || 0
      let end = 0
      for (let index = 0; index < video.buffered.length; index += 1) {
        end = Math.max(end, video.buffered.end(index))
      }
      setBufferedEnd(end)
    }
    const onLoaded = () => {
      updateDuration()
      const restore = resumePositionRef.current
      if (restore > 0 && Number.isFinite(video.duration)) {
        video.currentTime = Math.min(restore, Math.max(0, video.duration - 0.05))
      }
      video.playbackRate = rate
      setLoading(false)
      void video.play().catch(() => setPaused(true))
    }
    const onVideoError = () => {
      if (mode === 'file') {
        setLoading(false)
        setError('最终文件的编码不受内置播放器支持，可使用系统播放器打开')
      }
    }
    const onPlay = () => setPaused(false)
    const onPause = () => setPaused(true)
    const onWaiting = () => setLoading(true)
    const onPlaying = () => setLoading(false)
    video.addEventListener('loadedmetadata', onLoaded)
    video.addEventListener('durationchange', updateDuration)
    video.addEventListener('timeupdate', updateProgress)
    video.addEventListener('progress', updateProgress)
    video.addEventListener('play', onPlay)
    video.addEventListener('pause', onPause)
    video.addEventListener('waiting', onWaiting)
    video.addEventListener('playing', onPlaying)
    video.addEventListener('error', onVideoError)
    return () => {
      video.removeEventListener('loadedmetadata', onLoaded)
      video.removeEventListener('durationchange', updateDuration)
      video.removeEventListener('timeupdate', updateProgress)
      video.removeEventListener('progress', updateProgress)
      video.removeEventListener('play', onPlay)
      video.removeEventListener('pause', onPause)
      video.removeEventListener('waiting', onWaiting)
      video.removeEventListener('playing', onPlaying)
      video.removeEventListener('error', onVideoError)
    }
  }, [mode, rate])

  useEffect(() => {
    const onFullscreen = () => setFullscreen(Boolean(document.fullscreenElement))
    document.addEventListener('fullscreenchange', onFullscreen)
    return () => document.removeEventListener('fullscreenchange', onFullscreen)
  }, [])

  const togglePlay = useCallback(() => {
    const video = videoRef.current
    if (!video) return
    if (video.paused) void video.play(); else video.pause()
  }, [])

  const seekTo = useCallback((target: number) => {
    const video = videoRef.current
    if (!video || !Number.isFinite(target)) return
    const bounded = Math.max(0, Math.min(effectiveDuration || target, target))
    if (mode !== 'hls' || !session) {
      video.currentTime = bounded
      return
    }

    if (seekTimerRef.current) window.clearTimeout(seekTimerRef.current)
    const requestNumber = ++seekRequestRef.current
    setError('')
    setLoading(true)
    seekTimerRef.current = window.setTimeout(() => {
      void requestPlaybackSeek(task.id, session.session_id, bounded)
        .then(async result => {
          if (requestNumber !== seekRequestRef.current) return
          const hls = mainHlsRef.current
          hls?.stopLoad()
          hls?.startLoad(result.time)

          const deadline = Date.now() + 45_000
          while (requestNumber === seekRequestRef.current && Date.now() < deadline) {
            const currentVideo = videoRef.current
            if (currentVideo && isTimeSeekable(currentVideo.seekable, result.time)) {
              resumePositionRef.current = result.time
              currentVideo.currentTime = result.time
              setCurrentTime(result.time)
              setLoading(false)
              void currentVideo.play().catch(() => setPaused(true))
              return
            }
            await new Promise(resolve => window.setTimeout(resolve, 150))
          }
          if (requestNumber === seekRequestRef.current) {
            setLoading(false)
            setError('目标位置下载超时，请稍后重试')
          }
        })
        .catch(reason => {
          if (requestNumber === seekRequestRef.current) {
            setLoading(false)
            setError(reason?.message || '无法跳转到目标位置')
          }
        })
    }, 120)
  }, [effectiveDuration, mode, session, task.id])

  const seekBy = useCallback((seconds: number) => {
    const video = videoRef.current
    if (!video) return
    seekTo(Math.max(0, Math.min(effectiveDuration, video.currentTime + seconds)))
  }, [effectiveDuration, seekTo])

  const toggleMute = useCallback(() => {
    const video = videoRef.current
    if (!video) return
    video.muted = !video.muted
    setMuted(video.muted)
  }, [])

  const toggleFullscreen = useCallback(async () => {
    if (document.fullscreenElement) await document.exitFullscreen()
    else await containerRef.current?.requestFullscreen()
  }, [])

  const togglePictureInPicture = useCallback(async () => {
    const video = videoRef.current
    if (!video || !document.pictureInPictureEnabled) return
    if (document.pictureInPictureElement) await document.exitPictureInPicture()
    else await video.requestPictureInPicture()
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.target as HTMLElement)?.matches('input, select, textarea')) return
      if (event.key === 'Escape') { onClose(); return }
      if (event.key === ' ' || event.key.toLowerCase() === 'k') { event.preventDefault(); togglePlay() }
      if (event.key === 'ArrowLeft') { event.preventDefault(); seekBy(-10) }
      if (event.key === 'ArrowRight') { event.preventDefault(); seekBy(10) }
      if (event.key.toLowerCase() === 'm') toggleMute()
      if (event.key.toLowerCase() === 'f') void toggleFullscreen()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose, seekBy, toggleFullscreen, toggleMute, togglePlay])

  const ensureThumbnailSource = useCallback(async (targetTime: number) => {
    if (!session || !thumbnailVideoRef.current) throw new Error('播放器尚未准备好')
    const video = thumbnailVideoRef.current
    const sourceKey = `${mode}:${session.session_id}`
    if (thumbnailSourceRef.current === sourceKey && thumbnailReadyRef.current) {
      await thumbnailReadyRef.current
      return
    }

    destroyThumbnailDecoder()
    thumbnailSourceRef.current = sourceKey
    if (mode === 'file') {
      thumbnailReadyRef.current = (async () => {
        video.preload = 'auto'
        video.src = playbackMediaUrl(task.id, session.session_id)
        video.load()
        if (video.readyState < 1) await waitForMediaEvent(video, 'loadedmetadata')
      })()
    } else {
      if (!Hls.isSupported()) throw new Error('缩略图解码不可用')
      thumbnailReadyRef.current = new Promise<void>((resolve, reject) => {
        const hls = new Hls({
          startPosition: targetTime,
          maxBufferLength: 2,
          maxMaxBufferLength: 4,
          backBufferLength: 0,
          maxBufferSize: 3 * 1024 * 1024,
          xhrSetup(xhr) { xhr.setRequestHeader('X-Token', getToken()) },
        })
        thumbnailHlsRef.current = hls
        const timeout = window.setTimeout(() => reject(new Error('缩略图加载超时')), 6000)
        hls.on(Hls.Events.FRAG_BUFFERED, () => {
          window.clearTimeout(timeout)
          resolve()
        })
        hls.on(Hls.Events.ERROR, (_event, data) => {
          if (data.fatal) {
            window.clearTimeout(timeout)
            reject(new Error('缩略图分片无法读取'))
          }
        })
        hls.attachMedia(video)
        hls.on(Hls.Events.MEDIA_ATTACHED, () => {
          hls.loadSource(playbackPlaylistUrl(task.id, session.session_id, true))
        })
      })
    }
    try {
      await thumbnailReadyRef.current
    } catch (reason) {
      // Do not keep a rejected decoder promise: a later hover may succeed after
      // another segment has become available.
      if (thumbnailSourceRef.current === sourceKey) {
        destroyThumbnailDecoder()
      }
      throw reason
    }
  }, [destroyThumbnailDecoder, mode, session, task.id])

  const captureThumbnail = useCallback(async (time: number, key: number) => {
    if (thumbnailBusyRef.current) {
      pendingThumbnailRef.current = { time, key }
      return
    }
    thumbnailBusyRef.current = true
    try {
      await ensureThumbnailSource(time)
      const video = thumbnailVideoRef.current
      if (!video) return
      thumbnailHlsRef.current?.startLoad(time)
      if (Math.abs(video.currentTime - time) > 0.08) {
        const seeked = waitForMediaEvent(video, 'seeked')
        video.currentTime = time
        await seeked
      }
      if (video.readyState < 2) await waitForMediaEvent(video, 'loadeddata')
      const width = 184
      const ratio = video.videoWidth > 0 && video.videoHeight > 0
        ? video.videoHeight / video.videoWidth
        : 9 / 16
      const canvas = document.createElement('canvas')
      canvas.width = width
      canvas.height = Math.max(80, Math.round(width * ratio))
      canvas.getContext('2d')?.drawImage(video, 0, 0, canvas.width, canvas.height)
      const blob = await new Promise<Blob | null>(resolve => canvas.toBlob(resolve, 'image/jpeg', 0.72))
      if (!blob) return
      const image = URL.createObjectURL(blob)
      const cache = thumbnailCacheRef.current
      const old = cache.get(key)
      if (old) URL.revokeObjectURL(old)
      cache.set(key, image)
      while (cache.size > THUMBNAIL_CACHE_LIMIT) {
        const oldest = cache.entries().next().value as [number, string] | undefined
        if (!oldest) break
        URL.revokeObjectURL(oldest[1])
        cache.delete(oldest[0])
      }
      setHover(current => current?.key === key ? { ...current, image } : current)
      thumbnailHlsRef.current?.stopLoad()
    } catch {
      // A timestamp remains visible when a codec cannot produce a thumbnail.
    } finally {
      thumbnailBusyRef.current = false
      const pending = pendingThumbnailRef.current
      pendingThumbnailRef.current = null
      if (pending && pending.key !== key) void captureThumbnail(pending.time, pending.key)
    }
  }, [ensureThumbnailSource])

  const onTimelineMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const track = timelineRef.current
    if (!track || effectiveDuration <= 0) return
    if (thumbnailIdleRef.current) window.clearTimeout(thumbnailIdleRef.current)
    const rect = track.getBoundingClientRect()
    const time = timelineTime(event.clientX, rect.left, rect.width, effectiveDuration)
    const key = thumbnailBucket(time, effectiveDuration)
    const image = thumbnailCacheRef.current.get(key) || ''
    setHover({ time, key, image, left: thumbnailLeft(time, effectiveDuration, rect.width) })
    if (!image) {
      if (thumbnailTimerRef.current) window.clearTimeout(thumbnailTimerRef.current)
      thumbnailTimerRef.current = window.setTimeout(() => {
        void captureThumbnail(key, key)
      }, 180)
    }
  }

  const onTimelineLeave = () => {
    setHover(null)
    if (thumbnailTimerRef.current) window.clearTimeout(thumbnailTimerRef.current)
    thumbnailIdleRef.current = window.setTimeout(destroyThumbnailDecoder, 8000)
  }

  useEffect(() => () => {
    seekRequestRef.current += 1
    if (seekTimerRef.current) window.clearTimeout(seekTimerRef.current)
    if (thumbnailTimerRef.current) window.clearTimeout(thumbnailTimerRef.current)
    if (thumbnailIdleRef.current) window.clearTimeout(thumbnailIdleRef.current)
    destroyThumbnailDecoder()
    for (const image of thumbnailCacheRef.current.values()) URL.revokeObjectURL(image)
    thumbnailCacheRef.current.clear()
  }, [destroyThumbnailDecoder])

  const progress = effectiveDuration > 0 ? Math.min(100, currentTime / effectiveDuration * 100) : 0
  const buffered = effectiveDuration > 0 ? Math.min(100, bufferedEnd / effectiveDuration * 100) : 0
  const isDownloading = isRunningStatus(task.status)

  return <div className="modal-overlay player-overlay" onMouseDown={onClose}>
    <section ref={containerRef} className="player-modal" onMouseDown={event => event.stopPropagation()}>
      <header className="player-header">
        <div className="player-title"><strong>{task.title || task.filename || task.id}</strong><span>{mode === 'file' ? '本地文件' : statusLabel(task.status)}</span></div>
        <div className="player-header-stats">
          {isDownloading && <span className="player-speed"><Activity size={14} />{fmtSpeed(task.speed_bytes_per_sec)}</span>}
          <span>{mode === 'hls'
            ? `${formatPlayerTime(playbackStatus?.available_duration || task.playable_duration || 0)} / ${formatPlayerTime(effectiveDuration)} 可用`
            : '本地文件'}</span>
          <button className="player-icon-button" title="关闭播放器" onClick={onClose}><X size={19} /></button>
        </div>
      </header>

      <div className="player-stage" onDoubleClick={() => void toggleFullscreen()}>
        <video ref={videoRef} className="player-video" playsInline preload="metadata" />
        <video ref={thumbnailVideoRef} className="thumbnail-decoder" muted playsInline preload="none" />
        {loading && !error && <div className="player-loading"><LoaderCircle className="spin" size={28} /><span>{mode === 'hls' ? '正在读取播放清单并准备目标分片' : '正在打开本地文件'}</span></div>}
        {error && <div className="player-error"><strong>无法播放</strong><span>{error}</span></div>}

        <div className="player-controls" onDoubleClick={event => event.stopPropagation()}>
          <div ref={timelineRef} className="player-timeline" onPointerMove={onTimelineMove} onPointerLeave={onTimelineLeave}>
            {hover && <div className="timeline-preview" style={{ left: hover.left }}>
              <div>{hover.image ? <img src={hover.image} alt="" /> : <span className="thumbnail-loading"><LoaderCircle className="spin" size={18} /></span>}</div>
              <b>{formatPlayerTime(hover.time)}</b>
            </div>}
            <div className="timeline-track"><i className="timeline-buffered" style={{ width: `${buffered}%` }} /><i className="timeline-played" style={{ width: `${progress}%` }} /></div>
            <input
              aria-label="播放进度"
              type="range"
              min="0"
              max={Math.max(0.1, effectiveDuration)}
              step="0.05"
              value={Math.min(currentTime, Math.max(0.1, effectiveDuration))}
              onChange={event => seekTo(Number(event.target.value))}
            />
          </div>

          <div className="player-control-row">
            <div className="player-control-group">
              <button className="player-icon-button" title={paused ? '播放' : '暂停'} onClick={togglePlay}>{paused ? <Play size={20} fill="currentColor" /> : <Pause size={20} fill="currentColor" />}</button>
              <button className="player-icon-button" title="后退 10 秒" onClick={() => seekBy(-10)}><RotateCcw size={18} /></button>
              <button className="player-icon-button" title="前进 10 秒" onClick={() => seekBy(10)}><RotateCw size={18} /></button>
              <button className="player-icon-button" title={muted ? '取消静音' : '静音'} onClick={toggleMute}>{muted || volume === 0 ? <VolumeX size={19} /> : <Volume2 size={19} />}</button>
              <input className="volume-slider" aria-label="音量" type="range" min="0" max="1" step="0.05" value={muted ? 0 : volume} onChange={event => {
                const next = Number(event.target.value)
                setVolume(next); setMuted(next === 0)
                if (videoRef.current) { videoRef.current.volume = next; videoRef.current.muted = next === 0 }
              }} />
              <span className="player-time">{formatPlayerTime(currentTime)} / {formatPlayerTime(effectiveDuration)}</span>
            </div>
            <div className="player-control-group">
              {isDownloading && <span className="download-speed-display"><Activity size={14} />{fmtSpeed(task.speed_bytes_per_sec)}</span>}
              <label className="rate-control" title="播放速度"><Gauge size={16} /><select value={rate} onChange={event => {
                const next = Number(event.target.value)
                setRate(next)
                if (videoRef.current) videoRef.current.playbackRate = next
              }}>{PLAYBACK_RATES.map(value => <option key={value} value={value}>{value}×</option>)}</select></label>
              {document.pictureInPictureEnabled && <button className="player-icon-button" title="画中画" onClick={() => void togglePictureInPicture()}><PictureInPicture2 size={18} /></button>}
              <button className="player-icon-button" title={fullscreen ? '退出全屏' : '全屏'} onClick={() => void toggleFullscreen()}>{fullscreen ? <Minimize size={18} /> : <Maximize size={18} />}</button>
            </div>
          </div>
        </div>
      </div>
    </section>
  </div>
}
