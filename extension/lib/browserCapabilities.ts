export interface FilenameDeterminationEvent {
  addListener(listener: (...args: any[]) => void): void
}

export function filenameDeterminationEvent(
  isChrome: boolean,
  downloads: { onDeterminingFilename?: FilenameDeterminationEvent },
): FilenameDeterminationEvent | null {
  return isChrome && downloads.onDeterminingFilename?.addListener
    ? downloads.onDeterminingFilename
    : null
}

export function requestHeaderExtraInfo(isChrome: boolean): string[] {
  return isChrome ? ['requestHeaders', 'extraHeaders'] : ['requestHeaders']
}

export async function resolveFirefoxClickIntent<T>(
  cached: T | undefined,
  waitForIntent: () => Promise<T | undefined>,
): Promise<T | undefined> {
  return cached ?? waitForIntent()
}

export interface RecurringAlarmScheduler {
  create(name: string, info: { periodInMinutes?: number }): unknown
}

/** Firefox clamps recurring alarms to one minute; older Chromium builds throw on sub-minute periods. */
export const PORTABLE_RECURRING_ALARM_MINUTES = 1

/**
 * Create a recurring alarm with the requested period, falling back to the
 * portable one-minute period when the browser clamps sub-minute periods or
 * rejects the creation outright. Never throws: losing the alarm silently is
 * worse than running it on a slower cadence.
 */
export function createRecurringAlarm(
  alarms: RecurringAlarmScheduler,
  name: string,
  periodInMinutes: number,
  isFirefox = false,
): void {
  const requested = { periodInMinutes }
  const portable = { periodInMinutes: Math.max(PORTABLE_RECURRING_ALARM_MINUTES, periodInMinutes) }
  const attempts = isFirefox && periodInMinutes < PORTABLE_RECURRING_ALARM_MINUTES
    ? [portable, requested]
    : [requested, portable]
  for (const attempt of attempts) {
    try {
      alarms.create(name, attempt)
      return
    } catch {
      // Retry with the portable period before giving up entirely.
    }
  }
}
