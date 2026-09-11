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
  create(name: string, info: { periodInMinutes?: number }): unknown | Promise<unknown>
}

/** Firefox clamps recurring alarms to one minute; older Chromium builds throw on sub-minute periods. */
export const PORTABLE_RECURRING_ALARM_MINUTES = 1

/**
 * Create a recurring alarm with the requested period, falling back to the
 * portable one-minute period when the browser clamps sub-minute periods or
 * rejects the creation outright. Never throws: losing the alarm silently is
 * worse than running it on a slower cadence.
 */
export async function createRecurringAlarm(
  alarms: RecurringAlarmScheduler,
  name: string,
  periodInMinutes: number,
  isFirefox = false,
): Promise<void> {
  const requested = { periodInMinutes }
  const portable = { periodInMinutes: Math.max(PORTABLE_RECURRING_ALARM_MINUTES, periodInMinutes) }
  const firstAttempt = isFirefox && periodInMinutes < PORTABLE_RECURRING_ALARM_MINUTES
    ? portable
    : requested
  // Once Firefox has clamped a sub-minute request to the portable cadence,
  // never retry the known-unsupported original period after a transient
  // alarms.create failure. Repeating the portable request is the only useful
  // fallback in that browser.
  const attempts = [firstAttempt, portable]
  for (const attempt of attempts) {
    try {
      await alarms.create(name, attempt)
      return
    } catch {
      // Retry with the portable period before giving up entirely.
    }
  }
}
