export function compareNumericVersions(left: string, right: string): number {
  const parse = (value: string) => value.split('.').map(part => Number.parseInt(part, 10) || 0)
  const leftParts = parse(String(left || ''))
  const rightParts = parse(String(right || ''))
  const length = Math.max(leftParts.length, rightParts.length)
  for (let index = 0; index < length; index += 1) {
    const difference = (leftParts[index] || 0) - (rightParts[index] || 0)
    if (difference) return difference < 0 ? -1 : 1
  }
  return 0
}

export function extensionNeedsUpgrade(current: string, recommended: string): boolean {
  return Boolean(recommended) && compareNumericVersions(current, recommended) < 0
}
