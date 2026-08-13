export const COMPLETION_SOUND_COALESCE_MS = 700

let enabled = false
let lastPlayedAt = 0

export function setCompletionSoundEnabled(value: boolean): void {
  enabled = Boolean(value)
}

export function resetCompletionSoundState(): void {
  enabled = false
  lastPlayedAt = 0
}

export function shouldPlayCompletionSound(now = Date.now(), force = false): boolean {
  if (!force && !enabled) return false
  if (!force && now - lastPlayedAt < COMPLETION_SOUND_COALESCE_MS) return false
  if (!force) lastPlayedAt = now
  return true
}

function defaultPlay(): void {
  if (typeof window === 'undefined') return
  const Ctor = window.AudioContext || (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
  if (!Ctor) return
  const ctx = new Ctor()
  const start = ctx.currentTime
  const notes = [880, 1174.66]
  notes.forEach((frequency, index) => {
    const oscillator = ctx.createOscillator()
    const gain = ctx.createGain()
    const offset = index * 0.11
    oscillator.type = 'sine'
    oscillator.frequency.value = frequency
    gain.gain.setValueAtTime(0.0001, start + offset)
    gain.gain.exponentialRampToValueAtTime(0.07, start + offset + 0.02)
    gain.gain.exponentialRampToValueAtTime(0.0001, start + offset + 0.26)
    oscillator.connect(gain)
    gain.connect(ctx.destination)
    oscillator.start(start + offset)
    oscillator.stop(start + offset + 0.28)
  })
  void ctx.resume().catch(() => undefined)
  window.setTimeout(() => { void ctx.close().catch(() => undefined) }, 700)
}

export function playCompletionChime(force = false, now = Date.now(), play: () => void = defaultPlay): boolean {
  if (!shouldPlayCompletionSound(now, force)) return false
  try {
    play()
  } catch {
    return false
  }
  return true
}
