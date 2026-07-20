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
