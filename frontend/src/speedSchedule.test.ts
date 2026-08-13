import { describe, expect, it } from 'vitest'
import { effectiveSpeedLimitKib, insideSpeedWindow, parseHhmm } from './speedSchedule'

const settings = {
  download_speed_limit_kib: 512,
  speed_schedule_enabled: true,
  speed_schedule_start: '08:00',
  speed_schedule_end: '23:00',
  speed_schedule_limit_kib: 128,
}

describe('speed schedule chip', () => {
  it('parses HH:MM and rejects invalid values', () => {
    expect(parseHhmm('08:00')).toEqual([8, 0])
    expect(parseHhmm('25:00')).toBeNull()
    expect(parseHhmm('08:xx')).toBeNull()
  })

  it('treats the window as a half-open interval', () => {
    expect(insideSpeedWindow(new Date(2026, 7, 13, 8, 0), [8, 0], [23, 0])).toBe(true)
    expect(insideSpeedWindow(new Date(2026, 7, 13, 23, 0), [8, 0], [23, 0])).toBe(false)
    expect(insideSpeedWindow(new Date(2026, 7, 13, 23, 15), [22, 0], [8, 0])).toBe(true)
    expect(insideSpeedWindow(new Date(2026, 7, 13, 8, 0), [22, 0], [8, 0])).toBe(false)
  })

  it('uses the scheduled cap only inside the window', () => {
    expect(effectiveSpeedLimitKib(settings, new Date(2026, 7, 13, 12, 0))).toBe(128)
    expect(effectiveSpeedLimitKib(settings, new Date(2026, 7, 13, 23, 0))).toBe(512)
    expect(effectiveSpeedLimitKib({ ...settings, speed_schedule_enabled: false }, new Date(2026, 7, 13, 12, 0))).toBe(512)
    expect(effectiveSpeedLimitKib({ ...settings, speed_schedule_start: '08:00', speed_schedule_end: '08:00' }, new Date(2026, 7, 13, 12, 0))).toBe(512)
    expect(effectiveSpeedLimitKib({ ...settings, speed_schedule_limit_kib: 0 }, new Date(2026, 7, 13, 10, 0))).toBe(0)
  })
})
